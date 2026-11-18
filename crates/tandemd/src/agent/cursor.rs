//! El cursor en vivo en el agente (T-128): posición y cruces para la UI, solo mientras
//! alguien mira.
//!
//! El motor de entrada ([`crate::input`]) publica muestras (a 30 Hz como mucho) y cruces
//! cuando [`CursorGate`] está encendido, y el agente las reparte: a las conexiones de la UI
//! de aquí que pidieron [`tandem_proto::UiCommand::WatchCursor`] y, por el plano de control,
//! a la otra máquina si ella mira. Las muestras salen de quien tiene el foco, así que la UI
//! de la otra máquina las recibe por aquí ([`ControlMsg::Cursor`]); ese camino descarta lo
//! que se atrasa ([`PeerLink::send_lossy`](crate::orchestrator::PeerLink::send_lossy)).
//!
//! El interruptor se enciende con la primera suscripción (de aquí o de la otra máquina) y se
//! apaga con la última. El pedido de la otra máquina es un alquiler, como el de los
//! recursos ([`super::env`]): si deja de renovarse (el enlace cayó), se apaga solo.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tandem_input::session::{CursorEvent, CursorGate};
use tandem_proto::ControlMsg;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::Agent;
use super::env::{LEASE, RENEW};

/// Cuántos avisos espera una conexión lenta de la UI antes de empezar a perder los más
/// viejos.
const BACKLOG: usize = 64;

pub(super) struct CursorState {
    gate: Arc<CursorGate>,
    /// Lo que se publica aquí y lo que manda la otra máquina, para las conexiones de la UI.
    events: broadcast::Sender<CursorEvent>,
    local: Mutex<Local>,
    serving: Mutex<Serving>,
    /// Cuántos eventos salieron del motor de entrada (las pruebas lo miran).
    published: AtomicU64,
}

#[derive(Default)]
struct Local {
    /// Conexiones de la UI de aquí que miran, y la tarea que renueva el pedido a la otra.
    watchers: usize,
    renew: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Serving {
    /// Hasta cuándo la otra máquina quiere el cursor de esta.
    until: Option<Instant>,
    running: bool,
}

impl std::fmt::Debug for CursorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorState")
            .field("gate", &self.gate)
            .finish_non_exhaustive()
    }
}

impl Default for CursorState {
    fn default() -> Self {
        CursorState {
            gate: Arc::new(CursorGate::default()),
            events: broadcast::channel(BACKLOG).0,
            local: Mutex::new(Local::default()),
            serving: Mutex::new(Serving::default()),
            published: AtomicU64::new(0),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Agent {
    /// El interruptor que lee el motor de entrada: encendido mientras haya quien mire el
    /// cursor, de aquí o de la otra máquina.
    pub fn cursor_gate(&self) -> Arc<CursorGate> {
        self.cursor.gate.clone()
    }

    /// Cuántos eventos del cursor publicó el motor de entrada desde que arrancó el agente.
    pub fn cursor_published(&self) -> u64 {
        self.cursor.published.load(Ordering::Relaxed)
    }

    fn peer_watches_cursor(&self) -> bool {
        lock(&self.cursor.serving)
            .until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Enciende o apaga el interruptor según haya quien mire.
    fn refresh_cursor_gate(&self) {
        let local = lock(&self.cursor.local).watchers > 0;
        self.cursor.gate.set(local || self.peer_watches_cursor());
    }

    /// Una muestra o un cruce de este motor de entrada: va a las conexiones de aquí y, si
    /// la otra máquina mira, a ella.
    pub fn publish_cursor(&self, event: CursorEvent) {
        self.cursor.published.fetch_add(1, Ordering::Relaxed);
        if self.peer_watches_cursor() {
            match &event {
                CursorEvent::Sample(sample) => self
                    .orch
                    .peer()
                    .send_lossy(ControlMsg::Cursor(sample.clone())),
                // Un cruce es raro y la UI no lo puede perder: va por el camino de siempre.
                CursorEvent::Crossed { from, to, edge } => {
                    self.orch.peer().send(ControlMsg::Crossed {
                        from: *from,
                        to: *to,
                        edge: *edge,
                    });
                }
            }
        }
        // Sin conexiones de la UI no es un error.
        let _ = self.cursor.events.send(event);
    }

    /// Una muestra o un cruce que mandó la otra máquina, para quien mire aquí.
    pub fn receive_peer_cursor(&self, event: CursorEvent) {
        let _ = self.cursor.events.send(event);
    }

    /// Empezar a recibir el cursor de las dos máquinas. Mientras exista alguno se enciende
    /// el interruptor aquí y se le pide (y se renueva el pedido) a la otra máquina.
    pub fn watch_cursor(self: &Arc<Self>) -> CursorWatch {
        let events = self.cursor.events.subscribe();
        let mut l = lock(&self.cursor.local);
        l.watchers += 1;
        if l.watchers == 1
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            let weak = Arc::downgrade(self);
            l.renew = Some(rt.spawn(async move {
                loop {
                    match weak.upgrade() {
                        Some(a) => a.orch.peer().send(ControlMsg::WatchCursor { on: true }),
                        None => return,
                    }
                    tokio::time::sleep(RENEW).await;
                }
            }));
        }
        drop(l);
        self.refresh_cursor_gate();
        CursorWatch {
            agent: Arc::downgrade(self),
            events,
        }
    }

    fn unwatch_cursor(&self) {
        let mut l = lock(&self.cursor.local);
        l.watchers = l.watchers.saturating_sub(1);
        let last = l.watchers == 0;
        if last && let Some(task) = l.renew.take() {
            task.abort();
        }
        drop(l);
        self.refresh_cursor_gate();
        if last {
            self.orch.peer().send(ControlMsg::WatchCursor { on: false });
        }
    }

    /// La otra máquina quiere (o ya no) el cursor de esta.
    pub fn receive_watch_cursor(self: &Arc<Self>, on: bool) {
        let mut s = lock(&self.cursor.serving);
        if !on {
            s.until = None;
            drop(s);
            self.refresh_cursor_gate();
            return;
        }
        s.until = Some(Instant::now() + LEASE);
        let start = !std::mem::replace(&mut s.running, true);
        drop(s);
        self.refresh_cursor_gate();
        if !start {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            lock(&self.cursor.serving).running = false;
            return;
        };
        rt.spawn(lease(Arc::downgrade(self)));
    }
}

/// Qué hacer con el alquiler de la otra máquina en este instante.
enum Lease {
    /// Sigue vigente hasta ese momento.
    Until(Instant),
    /// Venció y ya quedó limpio.
    Expired,
}

/// Decide y limpia en una sola sección crítica: si decidiera y limpiara por separado, una
/// renovación entre ambas pasos se borraría y el interruptor quedaría apagado hasta la
/// siguiente (y `running` en falso la dejaría sin tarea que la vigile).
fn settle(serving: &Mutex<Serving>, now: Instant) -> Lease {
    let mut s = lock(serving);
    match s.until {
        Some(until) if now < until => Lease::Until(until),
        _ => {
            s.until = None;
            s.running = false;
            Lease::Expired
        }
    }
}

/// Vigila el alquiler de la otra máquina y apaga el interruptor cuando vence sin renovarse.
async fn lease(weak: Weak<Agent>) {
    loop {
        let Some(agent) = weak.upgrade() else { return };
        match settle(&agent.cursor.serving, Instant::now()) {
            Lease::Until(until) => {
                drop(agent);
                tokio::time::sleep_until(until).await;
            }
            Lease::Expired => {
                agent.refresh_cursor_gate();
                return;
            }
        }
    }
}

/// El cursor de las dos máquinas para una conexión de la UI. Al soltarlo se deja de mirar.
pub struct CursorWatch {
    agent: Weak<Agent>,
    events: broadcast::Receiver<CursorEvent>,
}

impl std::fmt::Debug for CursorWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CursorWatch")
    }
}

impl CursorWatch {
    /// El siguiente evento; `None` si el agente ya no existe. Si la conexión se atrasó,
    /// lo perdido se descarta: la próxima muestra ya dice dónde está el cursor. Se puede
    /// cancelar sin perder eventos.
    pub async fn next(&mut self) -> Option<CursorEvent> {
        loop {
            match self.events.recv().await {
                Ok(event) => return Some(event),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

impl Drop for CursorWatch {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.upgrade() {
            agent.unwatch_cursor();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use tandem_core::{Machine, Os, Side, State, Step};
    use tandem_proto::CursorSample;

    use super::*;
    use crate::StateStore;
    use crate::config::fixtures;
    use crate::orchestrator::{PeerLink, StepFuture};

    /// Anota lo que el agente le manda a la otra máquina, y por qué camino.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(bool, ControlMsg)>>);

    impl PeerLink for Recorder {
        fn execute<'a>(&'a self, _step: &'a Step) -> StepFuture<'a> {
            Box::pin(async { Ok(()) })
        }
        fn publish_state(&self, _state: &State) {}
        fn send(&self, msg: ControlMsg) {
            self.0.lock().unwrap().push((false, msg));
        }
        fn send_lossy(&self, msg: ControlMsg) {
            self.0.lock().unwrap().push((true, msg));
        }
    }

    impl Recorder {
        fn taken(&self) -> Vec<(bool, ControlMsg)> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    fn agent(dir: &std::path::Path) -> (Arc<Agent>, Arc<Recorder>) {
        let mut cfg = fixtures::desk();
        cfg.os = Os::MacOs;
        let a = Arc::new(Agent::new(cfg, StateStore::new(dir.join("state.json"))));
        let peer = Arc::new(Recorder::default());
        a.set_peer(peer.clone());
        peer.taken();
        (a, peer)
    }

    fn sample_of(x: i32) -> CursorSample {
        CursorSample {
            machine: Machine::A,
            screen: "mac-1".into(),
            x,
            y: 0,
        }
    }

    fn sample(x: i32) -> CursorEvent {
        CursorEvent::Sample(sample_of(x))
    }

    fn crossed() -> CursorEvent {
        CursorEvent::Crossed {
            from: Machine::A,
            to: Machine::B,
            edge: Side::Right,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn settle_keeps_a_renewed_lease_and_clears_an_expired_one_atomically() {
        let serving = Mutex::new(Serving {
            until: Some(Instant::now() + Duration::from_secs(5)),
            running: true,
        });
        // Renovado: ni se limpia `until` ni se baja `running`.
        let Lease::Until(until) = settle(&serving, Instant::now()) else {
            panic!("el alquiler seguía vigente");
        };
        assert!(lock(&serving).running);
        // Vencido: se limpia todo en la misma llamada, sin dejar `running` a medias.
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(matches!(settle(&serving, Instant::now()), Lease::Expired));
        let s = lock(&serving);
        assert!(s.until.is_none() && !s.running);
        assert!(until > Instant::now() - Duration::from_secs(6));
    }

    /// La carrera real (renovar entre leer y limpiar) solo ocurre entre hilos; aquí se
    /// castiga `settle` contra renovaciones concurrentes: nunca debe borrar una vigente.
    #[test]
    fn a_renewal_is_never_wiped_by_a_concurrent_settle() {
        let base = Instant::now();
        let far = std::time::Duration::from_secs(3600);
        for _ in 0..200 {
            let serving = Arc::new(Mutex::new(Serving {
                until: Some(base),
                running: true,
            }));
            let renewer = {
                let serving = serving.clone();
                std::thread::spawn(move || {
                    let mut s = lock(&serving);
                    s.until = Some(base + far);
                    s.running = true;
                })
            };
            let verdict = settle(&serving, base + Duration::from_secs(1));
            renewer.join().unwrap();
            let s = lock(&serving);
            // O vio la renovación (sigue vigente) o venció antes y la renovación llegó
            // después: en ambos casos la renovación sobrevive.
            assert!(
                s.until.is_some() && s.running,
                "{}",
                matches!(verdict, Lease::Expired)
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_gate_follows_the_local_watchers_and_the_other_machine_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (a, peer) = agent(dir.path());
        let gate = a.cursor_gate();
        assert!(!gate.is_on());

        let first = a.watch_cursor();
        let second = a.watch_cursor();
        assert!(gate.is_on());
        tokio::task::yield_now().await;
        // El pedido a la otra máquina sale una vez y se renueva mientras alguien mire.
        assert_eq!(
            peer.taken(),
            vec![(false, ControlMsg::WatchCursor { on: true })]
        );
        tokio::time::sleep(RENEW + Duration::from_millis(10)).await;
        assert_eq!(
            peer.taken(),
            vec![(false, ControlMsg::WatchCursor { on: true })]
        );

        // Con una conexión que se va, sigue habiendo quien mire.
        drop(first);
        assert!(gate.is_on());
        assert!(peer.taken().is_empty());
        drop(second);
        assert!(!gate.is_on());
        assert_eq!(
            peer.taken(),
            vec![(false, ControlMsg::WatchCursor { on: false })]
        );
        // Ya no se renueva.
        tokio::time::sleep(RENEW * 3).await;
        assert!(peer.taken().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_other_machine_rents_the_cursor_and_the_rent_expires() {
        let dir = tempfile::tempdir().unwrap();
        let (a, peer) = agent(dir.path());
        let gate = a.cursor_gate();

        // Nadie mira: lo que publique el motor no va a ningún lado.
        a.publish_cursor(sample(1));
        assert!(peer.taken().is_empty());

        a.receive_watch_cursor(true);
        assert!(gate.is_on());
        a.publish_cursor(sample(2));
        a.publish_cursor(crossed());
        // Las muestras van por el camino que se descarta; el cruce, por el normal.
        assert_eq!(
            peer.taken(),
            vec![
                (true, ControlMsg::Cursor(sample_of(2))),
                (
                    false,
                    ControlMsg::Crossed {
                        from: Machine::A,
                        to: Machine::B,
                        edge: Side::Right
                    }
                ),
            ]
        );

        // Renovado a tiempo, sigue; sin renovar, vence y el interruptor se apaga solo.
        tokio::time::sleep(LEASE - Duration::from_secs(1)).await;
        a.receive_watch_cursor(true);
        tokio::time::sleep(LEASE - Duration::from_secs(1)).await;
        assert!(gate.is_on(), "se renovó");
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!gate.is_on(), "venció sin renovar");
        a.publish_cursor(sample(3));
        assert!(peer.taken().is_empty());

        // Y se puede cancelar a mano.
        a.receive_watch_cursor(true);
        assert!(gate.is_on());
        a.receive_watch_cursor(false);
        assert!(!gate.is_on());
    }

    #[tokio::test(start_paused = true)]
    async fn local_watchers_get_both_machines_events_and_lost_ones_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _peer) = agent(dir.path());
        let mut watch = a.watch_cursor();

        a.publish_cursor(sample(1));
        a.receive_peer_cursor(crossed());
        assert_eq!(watch.next().await, Some(sample(1)));
        assert_eq!(watch.next().await, Some(crossed()));
        assert_eq!(a.cursor_published(), 1);

        // Una conexión que no lee se atrasa: pierde lo viejo y sigue con lo nuevo.
        for x in 0..(BACKLOG as i32 + 10) {
            a.receive_peer_cursor(sample(x));
        }
        let Some(CursorEvent::Sample(first)) = watch.next().await else {
            panic!("se esperaba una muestra");
        };
        assert!(first.x >= 10, "las más viejas se descartaron: {first:?}");
    }
}
