//! Concilia `pc_on` con los enlaces, en la Mac.
//!
//! Si la PC se queda sin corriente nadie corre un `PowerOffPc`: sin esto el agente de la
//! Mac seguiría creyéndola prendida y ningún plan volvería a mandar el Wake-on-LAN.
//! Cuando ningún enlace la ve durante [`crate::PowerConfig::pc_lost_after`], se la da
//! por apagada: `pc_on = false`, la Mac recupera lo que puede sola y la UI recibe
//! [`PowerEvent::PcLost`] con la razón. Una caída más corta no cambia nada.
//!
//! Lo que la Mac puede recuperar sin la PC es poco: las entradas las cambia la PC
//! (DESIGN §8), así que los monitores que estaban en su entrada siguen ahí, en negro,
//! y el estado lo dice (siguen siendo de la PC). La Mac recalcula la geometría del
//! teclado y mouse para que el cursor no vaya a pantallas apagadas; la pantalla
//! integrada ya sigue la regla de [`tandem_core::State::builtin_effective`]. El
//! siguiente cambio que necesite a la PC empieza por encenderla.
//!
//! Al revés, si la PC vuelve a verse con `pc_on = false` (la encendieron a mano tras
//! el corte, o la Mac arrancó sin saberlo), se la da por prendida.
//!
//! Nada de esto pisa una transacción: se toma el orquestador como un cambio más, así
//! que un apagado en curso termina antes (y deja `pc_on = false` él mismo).

use std::sync::Arc;
use std::time::Duration;

use tandem_core::{Machine, MonitorId, Step};
use tandem_proto::PowerEvent;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::agent::Agent;

/// Cada cuánto se vuelve a intentar si el orquestador está ocupado con un cambio.
pub const BUSY_RETRY: Duration = Duration::from_secs(1);

/// Vigila los enlaces con la PC hasta que se cierra `reachable` o el agente.
///
/// `reachable` es verdadero mientras algún enlace ve al agente de la PC (en el agente,
/// el enlace activo del gestor de enlaces). En la PC no hace nada: que la Mac no
/// conteste no dice nada de la corriente de la PC.
pub async fn watch(agent: Arc<Agent>, mut reachable: watch::Receiver<bool>) {
    if agent.machine() != Machine::Mac {
        return;
    }
    let lost_after = agent.config().power.pc_lost_after();
    let mut states = agent.subscribe();
    let mut up = *reachable.borrow_and_update();
    let mut down_since = (!up).then(Instant::now);
    // La PC se vio aparecer y todavía no se comprobó `pc_on`.
    let mut appeared = up;
    loop {
        let pc_on = agent.state().pc_on;
        let mut wake = None;
        if up {
            if appeared && !pc_on {
                if mark_on(&agent) {
                    appeared = false;
                    continue;
                }
                wake = Some(Instant::now() + BUSY_RETRY);
            } else {
                appeared = false;
            }
        } else if let Some(since) = down_since
            && pc_on
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

/// Da la PC por apagada. `false` si el orquestador está ocupado con un cambio.
async fn mark_lost(agent: &Agent, since: Instant) -> bool {
    if agent.orchestrator().try_begin().is_none() {
        return false;
    }
    let held = Held(agent);
    let mut state = agent.state();
    // El cambio que terminaba mientras se esperaba pudo haberla apagado.
    if !state.pc_on {
        return true;
    }
    let cfg = &agent.config().desk;
    let stranded: Vec<MonitorId> = state.monitors_of(cfg, Machine::Pc).cloned().collect();
    state.pc_on = false;
    let secs = since.elapsed().as_secs();
    let mut reason = format!(
        "la PC no responde por ningún enlace desde hace {secs} s sin que nadie la apagara \
         (¿corte de corriente?): se da por apagada"
    );
    if !stranded.is_empty() {
        let names: Vec<&str> = stranded
            .iter()
            .map(|id| cfg.monitor(id).map_or(id.0.as_str(), |m| m.label.as_str()))
            .collect();
        reason.push_str(&format!(
            "; {} {} en negro hasta encenderla",
            names.join(", "),
            if names.len() == 1 { "queda" } else { "quedan" }
        ));
    }
    tracing::warn!("{reason}");
    // Primero el estado, después el aviso: quien lo reciba ya ve la PC apagada.
    if let Err(e) = agent.set_state(state) {
        tracing::warn!("{e}");
    }
    agent.emit_power(PowerEvent::PcLost { reason, stranded });
    // Solo la Mac: la PC no puede contestar.
    if let Err(e) = agent
        .orchestrator()
        .run_here(&Step::UpdateInputGeometry)
        .await
    {
        tracing::warn!("tras perder la PC: {e}");
    }
    drop(held);
    true
}

/// Da la PC por prendida. `false` si el orquestador está ocupado con un cambio.
fn mark_on(agent: &Agent) -> bool {
    if agent.orchestrator().try_begin().is_none() {
        return false;
    }
    let _held = Held(agent);
    let mut state = agent.state();
    if !state.pc_on {
        tracing::info!("la PC volvió a responder: se da por prendida");
        state.pc_on = true;
        if let Err(e) = agent.set_state(state) {
            tracing::warn!("{e}");
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tandem_core::State;
    use tokio::sync::broadcast;

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::tests::{FakePeer, FakeSteps};
    use crate::store::StateStore;

    const LOST_AFTER: Duration = Duration::from_secs(15);

    struct Rig {
        agent: Arc<Agent>,
        local: Arc<FakeSteps>,
        peer: Arc<FakeSteps>,
        up: watch::Sender<bool>,
        events: broadcast::Receiver<PowerEvent>,
        task: tokio::task::JoinHandle<()>,
    }

    /// La Mac en Compartido (el 34" en la PC), con la PC visible según `up`.
    fn rig(dir: &Path, machine: Machine, pc_on: bool, up: bool) -> Rig {
        let mut cfg = fixtures::desk();
        cfg.machine = machine;
        let local = Arc::new(FakeSteps::default());
        let peer = Arc::new(FakeSteps::default());
        let agent =
            Agent::with_local_steps(cfg, StateStore::new(dir.join("state.json")), local.clone());
        agent.set_peer(Arc::new(FakePeer(peer.clone())));
        let mut s = agent.state();
        s.pc_on = pc_on;
        if pc_on {
            s.owner.insert("m34".into(), Machine::Pc);
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

    async fn sleep(secs: f64) {
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_drop_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = rig(dir.path(), Machine::Mac, true, true);
        let before = r.agent.state();
        for _ in 0..5 {
            r.up.send_replace(false);
            sleep(LOST_AFTER.as_secs_f64() - 1.0).await;
            r.up.send_replace(true);
            sleep(2.0).await;
        }
        sleep(60.0).await;
        assert_eq!(r.agent.state(), before);
        assert!(r.events.try_recv().is_err());
        assert!(r.local.steps().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn after_the_threshold_the_pc_is_off_and_the_ui_is_told_why() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = rig(dir.path(), Machine::Mac, true, true);
        let before = r.agent.state();
        r.up.send_replace(false);
        sleep(LOST_AFTER.as_secs_f64() - 0.1).await;
        assert!(r.agent.state().pc_on, "todavía dentro del umbral");
        sleep(0.2).await;

        let mut want = before.clone();
        want.pc_on = false;
        // El 34" sigue en la entrada de la PC: la Mac no puede cambiarlo sin ella.
        assert_eq!(r.agent.state(), want);
        let PowerEvent::PcLost { reason, stranded } = r.events.try_recv().unwrap();
        assert_eq!(stranded, [MonitorId::from("m34")]);
        assert!(reason.contains("15 s"), "{reason}");
        assert!(reason.contains("34\""), "{reason}");
        // La Mac recalcula su geometría sola; a la PC no se le pide nada.
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

    #[tokio::test(start_paused = true)]
    async fn a_change_in_progress_is_not_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = rig(dir.path(), Machine::Mac, true, true);
        r.agent.orchestrator().try_begin().unwrap();
        r.up.send_replace(false);
        sleep(LOST_AFTER.as_secs_f64() + 30.0).await;
        assert!(r.agent.state().pc_on);
        assert!(r.events.try_recv().is_err());
        r.agent.orchestrator().end();
        sleep(BUSY_RETRY.as_secs_f64() + 0.1).await;
        assert!(!r.agent.state().pc_on);
        assert!(r.events.try_recv().is_ok());
    }

    /// Un apagado en curso termina con `pc_on = false`: no hay corte que avisar.
    #[tokio::test(start_paused = true)]
    async fn a_shutdown_in_progress_is_not_a_power_cut() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = rig(dir.path(), Machine::Mac, true, true);
        r.agent.orchestrator().try_begin().unwrap();
        r.up.send_replace(false);
        sleep(LOST_AFTER.as_secs_f64() + 5.0).await;
        let mut s = r.agent.state();
        s.pc_on = false;
        s.owner.insert("m34".into(), Machine::Mac);
        r.agent.set_state(s.clone()).unwrap();
        r.agent.orchestrator().end();
        sleep(60.0).await;
        assert_eq!(r.agent.state(), s);
        assert!(r.events.try_recv().is_err());
        assert!(r.local.steps().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn with_the_pc_already_off_nothing_happens() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = rig(dir.path(), Machine::Mac, false, false);
        let before = r.agent.state();
        sleep(120.0).await;
        assert_eq!(r.agent.state(), before);
        assert!(r.events.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn the_pc_coming_back_is_on_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = rig(dir.path(), Machine::Mac, true, true);
        r.up.send_replace(false);
        sleep(LOST_AFTER.as_secs_f64() + 1.0).await;
        assert!(!r.agent.state().pc_on);
        r.events.try_recv().unwrap();

        // La encendieron a mano: Windows arranca con el 34" en DisplayPort.
        r.up.send_replace(true);
        sleep(0.1).await;
        let s = r.agent.state();
        assert!(s.pc_on);
        assert_eq!(s.owner[&MonitorId::from("m34")], Machine::Pc);
        assert!(r.events.try_recv().is_err());
    }

    /// La Mac arranca con el estado inicial (PC apagada) y la PC ya estaba prendida.
    #[tokio::test(start_paused = true)]
    async fn a_pc_seen_at_startup_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let r = rig(dir.path(), Machine::Mac, false, true);
        sleep(0.1).await;
        let mut want = State::initial(&r.agent.config().desk);
        want.pc_on = true;
        assert_eq!(r.agent.state(), want);
    }

    /// Si la PC aparece en medio de un cambio (encendiéndola), se espera a que termine.
    #[tokio::test(start_paused = true)]
    async fn the_pc_appearing_during_a_change_waits_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let r = rig(dir.path(), Machine::Mac, false, false);
        r.agent.orchestrator().try_begin().unwrap();
        r.up.send_replace(true);
        sleep(5.0).await;
        assert!(!r.agent.state().pc_on);
        r.agent.orchestrator().end();
        sleep(BUSY_RETRY.as_secs_f64() + 0.1).await;
        assert!(r.agent.state().pc_on);
    }

    #[tokio::test(start_paused = true)]
    async fn the_pc_does_not_watch_the_mac() {
        let dir = tempfile::tempdir().unwrap();
        let r = rig(dir.path(), Machine::Pc, true, false);
        let before = r.agent.state();
        r.task.await.unwrap();
        sleep(120.0).await;
        assert_eq!(r.agent.state(), before);
    }

    #[tokio::test(start_paused = true)]
    async fn ends_when_the_link_source_goes_away() {
        let dir = tempfile::tempdir().unwrap();
        let r = rig(dir.path(), Machine::Mac, true, true);
        drop(r.up);
        r.task.await.unwrap();
    }
}
