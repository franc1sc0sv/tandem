//! Concilia el encendido del par con los enlaces, en las dos máquinas (DESIGN §3.7).
//!
//! Si la otra máquina se queda sin corriente (o se suspende) nadie corre su `PowerOff`:
//! sin esto este agente la seguiría creyendo prendida y ningún plan volvería a mandar el
//! Wake-on-LAN. Cuando ningún enlace la ve durante [`crate::PowerConfig::peer_lost_after`],
//! se la da por apagada: `on` la marca apagada, esta máquina recupera lo que puede sola y
//! la UI recibe [`PowerEvent::PeerLost`] con la razón. Una caída más corta no cambia nada.
//!
//! Lo que se puede recuperar sin la otra es poco: los monitores que estaban en su entrada
//! siguen ahí, en negro, y el estado lo dice (siguen siendo suyos). Esta máquina recalcula
//! la geometría del teclado y mouse para que el cursor no vaya a pantallas apagadas; la
//! pantalla integrada ya sigue la regla de [`tandem_core::State::builtin_effective`]. El
//! siguiente cambio lleva los monitores a donde se pida: si los cambia esta máquina no
//! hace falta la otra; si los cambia la otra, empieza por encenderla.
//!
//! Al revés, si la otra vuelve a verse estando apagada (la encendieron a mano, se
//! despertó, o este agente arrancó sin saberlo), se la da por prendida.
//!
//! Nada de esto pisa una transacción: se toma el orquestador como un cambio más, así
//! que un apagado en curso termina antes (y la deja apagada él mismo).

use std::sync::Arc;
use std::time::Duration;

use tandem_core::{MonitorId, Step};
use tandem_proto::PowerEvent;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::agent::Agent;

/// Cada cuánto se vuelve a intentar si el orquestador está ocupado con un cambio.
pub const BUSY_RETRY: Duration = Duration::from_secs(1);

/// Vigila los enlaces con la otra máquina hasta que se cierra `reachable` o el agente.
///
/// `reachable` es verdadero mientras algún enlace ve al agente de la otra (en el agente,
/// el enlace activo del gestor de enlaces).
pub async fn watch(agent: Arc<Agent>, mut reachable: watch::Receiver<bool>) {
    let peer = agent.machine().other();
    let lost_after = agent.config().power.peer_lost_after();
    let mut states = agent.subscribe();
    let mut up = *reachable.borrow_and_update();
    let mut down_since = (!up).then(Instant::now);
    // La otra se vio aparecer y todavía no se comprobó si estaba prendida.
    let mut appeared = up;
    loop {
        let peer_on = agent.state().is_on(peer);
        let mut wake = None;
        if up {
            if appeared && !peer_on {
                if mark_on(&agent) {
                    appeared = false;
                    continue;
                }
                wake = Some(Instant::now() + BUSY_RETRY);
            } else {
                appeared = false;
            }
        } else if let Some(since) = down_since
            && peer_on
        {
            let due = since + lost_after;
            if Instant::now() >= due {
                if mark_lost(&agent, since).await {
                    continue;
                }
                wake = Some(Instant::now() + BUSY_RETRY);
            } else {
                wake = Some(due);
            }
        }
        tokio::select! {
            changed = reachable.changed() => {
                if changed.is_err() {
                    return;
                }
                let now_up = *reachable.borrow_and_update();
                if now_up != up {
                    up = now_up;
                    down_since = (!up).then(Instant::now);
                    appeared = up;
                }
            }
            changed = states.changed() => {
                if changed.is_err() {
                    return;
                }
                states.mark_unchanged();
            }
            () = sleep_until(wake), if wake.is_some() => {}
        }
    }
}

async fn sleep_until(at: Option<Instant>) {
    if let Some(at) = at {
        tokio::time::sleep_until(at).await;
    }
}

/// Libera el orquestador al terminar, aunque algo entre en pánico.
struct Held<'a>(&'a Agent);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.orchestrator().end();
    }
}

/// Da la otra por apagada. `false` si el orquestador está ocupado con un cambio.
async fn mark_lost(agent: &Agent, since: Instant) -> bool {
    if agent.orchestrator().try_begin().is_none() {
        return false;
    }
    let held = Held(agent);
    let peer = agent.machine().other();
    let mut state = agent.state();
    // El cambio que terminaba mientras se esperaba pudo haberla apagado.
    if !state.is_on(peer) {
        return true;
    }
    let cfg = agent.desk();
    let stranded: Vec<MonitorId> = state.monitors_of(&cfg, peer).cloned().collect();
    state.on.set(peer, false);
    let secs = since.elapsed().as_secs();
    let name = crate::capabilities::label(peer);
    let mut reason = format!(
        "La {name} no responde por ningún enlace desde hace {secs} s sin que nadie la apagara \
         (¿corte de corriente o suspendida?): se da por apagada"
    );
    if !stranded.is_empty() {
        let names: Vec<&str> = stranded
            .iter()
            .map(|id| cfg.monitor(id).map_or(id.0.as_str(), |m| m.label.as_str()))
            .collect();
        reason.push_str(&format!(
            "; {} {} en negro hasta llevarlos a otra máquina o encenderla",
            names.join(", "),
            if names.len() == 1 { "queda" } else { "quedan" }
        ));
    }
    tracing::warn!("{reason}");
    // Primero el estado, después el aviso: quien lo reciba ya la ve apagada.
    if let Err(e) = agent.set_state(state) {
        tracing::warn!("{e}");
    }
    agent.emit_power(PowerEvent::PeerLost {
        machine: peer,
        reason,
        stranded,
    });
    // Solo aquí: la otra no puede contestar.
    if let Err(e) = agent
        .orchestrator()
        .run_here(&Step::UpdateInputGeometry)
        .await
    {
        tracing::warn!("tras perder a {peer}: {e}");
    }
    drop(held);
    true
}

/// Da la otra por prendida. `false` si hay un cambio en curso: lo deja terminar.
///
/// No reserva el orquestador: la otra acaba de aparecer y puede estar pidiendo ya su
/// primer paso, que con el orquestador reservado se rechazaría.
fn mark_on(agent: &Agent) -> bool {
    if !agent.orchestrator().is_idle() {
        return false;
    }
    let peer = agent.machine().other();
    let mut state = agent.state();
    if !state.is_on(peer) {
        tracing::info!("{peer} volvió a responder: se da por prendida");
        state.on.set(peer, true);
        if let Err(e) = agent.set_state(state) {
            tracing::warn!("{e}");
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tandem_core::{Machine, State};
    use tokio::sync::broadcast;

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::tests::{FakePeer, FakeSteps};
    use crate::store::StateStore;

    const LOST_AFTER: Duration = Duration::from_secs(15);
    const BOTH: [Machine; 2] = [Machine::Mac, Machine::Pc];

    struct Rig {
        agent: Arc<Agent>,
        local: Arc<FakeSteps>,
        peer: Arc<FakeSteps>,
        up: watch::Sender<bool>,
        events: broadcast::Receiver<PowerEvent>,
        task: tokio::task::JoinHandle<()>,
    }

    /// El agente de `me` en Compartido (el 34" en la otra máquina), con la otra
    /// prendida o no y visible según `up`. Las mismas pruebas valen desde las dos
    /// máquinas: no hay una que vigile y otra vigilada.
    fn rig(dir: &Path, me: Machine, peer_on: bool, up: bool) -> Rig {
        let mut cfg = fixtures::desk();
        cfg.machine = me;
        let local = Arc::new(FakeSteps::default());
        let peer = Arc::new(FakeSteps::default());
        let agent =
            Agent::with_local_steps(cfg, StateStore::new(dir.join("state.json")), local.clone());
        agent.set_peer(Arc::new(FakePeer(peer.clone())));
        let mut s = agent.state();
        s.owner.values_mut().for_each(|o| *o = me);
        s.on.set(me.other(), peer_on);
        if peer_on {
            s.owner.insert("m34".into(), me.other());
        }
        agent.set_state(s).unwrap();
        let agent = Arc::new(agent);
        let (up, rx) = watch::channel(up);
        let events = agent.subscribe_power();
        let task = tokio::spawn(watch(agent.clone(), rx));
        Rig {
            agent,
            local,
            peer,
            up,
            events,
            task,
        }
    }

    fn peer_on(r: &Rig) -> bool {
        r.agent.state().is_on(r.agent.machine().other())
    }

    async fn sleep(secs: f64) {
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_drop_changes_nothing() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            let before = r.agent.state();
            for _ in 0..5 {
                r.up.send_replace(false);
                sleep(LOST_AFTER.as_secs_f64() - 1.0).await;
                r.up.send_replace(true);
                sleep(2.0).await;
            }
            sleep(60.0).await;
            assert_eq!(r.agent.state(), before, "{me}");
            assert!(r.events.try_recv().is_err());
            assert!(r.local.steps().is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn after_the_threshold_the_peer_is_off_and_the_ui_is_told_why() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            let before = r.agent.state();
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() - 0.1).await;
            assert!(peer_on(&r), "{me}: todavía dentro del umbral");
            sleep(0.2).await;

            let mut want = before.clone();
            want.on.set(me.other(), false);
            // El 34" sigue en la entrada de la otra; esta máquina sigue encendida.
            assert_eq!(r.agent.state(), want, "{me}");
            assert!(
                r.agent.state().is_on(me),
                "{me}: nunca apagada para sí misma"
            );
            let PowerEvent::PeerLost {
                machine,
                reason,
                stranded,
            } = r.events.try_recv().unwrap();
            assert_eq!(machine, me.other());
            assert_eq!(stranded, [MonitorId::from("m34")]);
            assert!(reason.contains("15 s"), "{reason}");
            assert!(reason.contains("34\""), "{reason}");
            assert!(
                reason.starts_with(&format!(
                    "La {} no responde",
                    crate::capabilities::label(me.other())
                )),
                "{reason}"
            );
            // Recalcula su geometría sola; a la otra no se le pide nada.
            assert_eq!(r.local.steps(), [Step::UpdateInputGeometry]);
            assert!(r.peer.steps().is_empty());
            // Y queda guardado.
            let saved = StateStore::new(dir.path().join("state.json"))
                .load()
                .unwrap();
            assert_eq!(saved, Some(want));
            // El orquestador quedó libre.
            assert!(r.agent.orchestrator().try_begin().is_some());

            // Sigue sin verse: no se repite el aviso.
            r.agent.orchestrator().end();
            sleep(120.0).await;
            assert!(r.events.try_recv().is_err());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_in_progress_is_not_interrupted() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            r.agent.orchestrator().try_begin().unwrap();
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() + 30.0).await;
            assert!(peer_on(&r));
            assert!(r.events.try_recv().is_err());
            r.agent.orchestrator().end();
            sleep(BUSY_RETRY.as_secs_f64() + 0.1).await;
            assert!(!peer_on(&r), "{me}");
            assert!(r.events.try_recv().is_ok());
        }
    }

    /// Un apagado en curso la deja apagada él mismo: no hay corte que avisar.
    #[tokio::test(start_paused = true)]
    async fn a_shutdown_in_progress_is_not_a_power_cut() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            r.agent.orchestrator().try_begin().unwrap();
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() + 5.0).await;
            let mut s = r.agent.state();
            s.on.set(me.other(), false);
            s.owner.insert("m34".into(), me);
            r.agent.set_state(s.clone()).unwrap();
            r.agent.orchestrator().end();
            sleep(60.0).await;
            assert_eq!(r.agent.state(), s);
            assert!(r.events.try_recv().is_err());
            assert!(r.local.steps().is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn with_the_peer_already_off_nothing_happens() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, false, false);
            let before = r.agent.state();
            sleep(120.0).await;
            assert_eq!(r.agent.state(), before);
            assert!(r.events.try_recv().is_err());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_peer_coming_back_is_on_again() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut r = rig(dir.path(), me, true, true);
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() + 1.0).await;
            assert!(!peer_on(&r));
            r.events.try_recv().unwrap();

            // La encendieron a mano (o se despertó) con el 34" en su entrada.
            r.up.send_replace(true);
            sleep(0.1).await;
            let s = r.agent.state();
            assert!(s.is_on(me.other()), "{me}");
            assert_eq!(s.owner[&MonitorId::from("m34")], me.other());
            assert!(r.events.try_recv().is_err());
        }
    }

    /// El agente arranca con el estado inicial (la otra apagada) y la otra ya estaba prendida.
    #[tokio::test(start_paused = true)]
    async fn a_peer_seen_at_startup_is_on() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = fixtures::desk();
            cfg.machine = me;
            let agent = Arc::new(Agent::new(
                cfg,
                StateStore::new(dir.path().join("state.json")),
            ));
            let (_up, rx) = watch::channel(true);
            let task = tokio::spawn(watch(agent.clone(), rx));
            sleep(0.1).await;
            let mut want = State::initial(&agent.desk(), me);
            want.on.set(me.other(), true);
            assert_eq!(agent.state(), want, "{me}");
            task.abort();
        }
    }

    /// Si la otra aparece en medio de un cambio (encendiéndola), se espera a que termine.
    #[tokio::test(start_paused = true)]
    async fn the_peer_appearing_during_a_change_waits_for_it() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let r = rig(dir.path(), me, false, false);
            r.agent.orchestrator().try_begin().unwrap();
            r.up.send_replace(true);
            sleep(5.0).await;
            assert!(!peer_on(&r));
            r.agent.orchestrator().end();
            sleep(BUSY_RETRY.as_secs_f64() + 0.1).await;
            assert!(peer_on(&r), "{me}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ends_when_the_link_source_goes_away() {
        for me in BOTH {
            let dir = tempfile::tempdir().unwrap();
            let r = rig(dir.path(), me, true, true);
            drop(r.up);
            r.task.await.unwrap();
        }
    }
}
