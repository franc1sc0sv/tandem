//! La calibración DDC desde el agente (T-078): la sonda «¿ves esto?» y el [`Confirmer`]
//! del motor de T-077 que la usa.
//!
//! La única prueba fiable de que un código de `0x60` funcionó es que alguien vea la
//! máquina destino en el monitor. Por eso la sonda se muestra en **esa** máquina, a
//! pantalla completa en el monitor que se calibra (buscado por su EDID):
//!
//! - Si la entrada que debería verse es la de esta máquina, la muestra su propia UI.
//! - Si es la de la otra, se le pide por el plano de control ([`PeerLink::show_probe`]):
//!   su agente la muestra en su UI y contesta si hubo clic antes del plazo.
//! - Sin la otra máquina (apagada, sin emparejar o sin UI abierta), respaldo local: esta
//!   UI pregunta «¿el monitor cambió?» con Sí/No.
//!
//! El plazo lo cumple quien muestra la sonda: sin clic a tiempo cuenta como no visto y
//! la sonda desaparece, aunque se pierda el aviso de quien calibra.
//!
//! [`PeerLink::show_probe`]: crate::orchestrator::PeerLink::show_probe

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tandem_core::Input;
use tandem_display::calibrate::{ConfirmError, ConfirmFuture, Confirmer, Probe, ProbeKind};
use tandem_display::desktop::DesktopControl;
use tandem_display::topology::{Bounds, EdidKey};
use tandem_proto::{
    CalibrationProbe, EdidId, ProbeAsk, ProbeEvent, ProbeMonitor, ProbeOutcome, ProbePlace,
    ProbeScreen,
};
use tokio::sync::{broadcast, oneshot};

use crate::agent::Agent;

/// Cada cuánto se vuelve a buscar el monitor mientras la sonda está a la vista: al
/// cambiarle la entrada, el sistema puede tardar en ponerlo en el escritorio.
pub const LOCATE_EVERY: Duration = Duration::from_millis(500);
/// Lo que se contesta si nadie puede ver la sonda en esta máquina.
pub const NO_UI: &str = "no hay una ventana de Tandem abierta para mostrar la sonda";

/// Avisos guardados para una UI lenta.
const EVENT_BUFFER: usize = 32;

/// Dónde está un monitor en el escritorio de esta máquina. Puede bloquear (consulta al
/// sistema): se llama fuera del runtime.
pub trait ScreenLocator: Send + Sync {
    fn locate(&self, key: &EdidKey) -> Result<Option<Bounds>, String>;
}

/// El escritorio real de la máquina ([`tandem_display::desktop::native`]).
pub struct DesktopLocator(pub Arc<dyn DesktopControl>);

impl ScreenLocator for DesktopLocator {
    fn locate(&self, key: &EdidKey) -> Result<Option<Bounds>, String> {
        self.0.locate(key).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no hay una sonda de calibración pendiente con el número {0}")]
pub struct UnknownProbe(pub u64);

/// Las sondas que muestra la UI de esta máquina y sus respuestas.
pub struct ProbeBoard {
    events: broadcast::Sender<ProbeEvent>,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    next: AtomicU64,
    locator: Option<Arc<dyn ScreenLocator>>,
}

impl std::fmt::Debug for ProbeBoard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeBoard")
            .field("locator", &self.locator.is_some())
            .finish_non_exhaustive()
    }
}

/// Quita la sonda y avisa a la UI al terminar, también si se suelta el futuro (venció
/// en quien calibra, se canceló o se cortó la conexión).
struct Shown<'a> {
    board: &'a ProbeBoard,
    id: u64,
}

impl Drop for Shown<'_> {
    fn drop(&mut self) {
        self.board.lock().remove(&self.id);
        let _ = self.board.events.send(ProbeEvent::Hide { id: self.id });
    }
}

impl ProbeBoard {
    /// Sin `locator` la UI busca el monitor por el nombre del EDID.
    pub fn new(locator: Option<Arc<dyn ScreenLocator>>) -> Self {
        ProbeBoard {
            events: broadcast::channel(EVENT_BUFFER).0,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            locator,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProbeEvent> {
        self.events.subscribe()
    }

    /// La respuesta del usuario: `seen` si hizo clic en la sonda (o dijo que sí).
    pub fn answer(&self, id: u64, seen: bool) -> Result<(), UnknownProbe> {
        let tx = self.lock().remove(&id).ok_or(UnknownProbe(id))?;
        // Si justo venció, nadie espera la respuesta; el cierre ya lo avisa la sonda.
        let _ = tx.send(seen);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<u64, oneshot::Sender<bool>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Muestra la sonda y espera la respuesta, a lo sumo [`CalibrationProbe::timeout_ms`]:
    /// sin clic a tiempo es [`ProbeOutcome::Unseen`] y la sonda se quita. `question` es
    /// el respaldo sin la otra máquina: la ventana pregunta en vez de cubrir el monitor.
    pub async fn show(&self, probe: &CalibrationProbe, question: bool) -> ProbeOutcome {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(probe.timeout_ms);
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = oneshot::channel();
        self.lock().insert(id, tx);
        let _shown = Shown { board: self, id };

        // En la pregunta no hace falta saber dónde está el monitor: no se mira en él.
        let key = (!question).then(|| edid_key(&probe.monitor)).flatten();
        let locator = key.as_ref().and(self.locator.clone());
        let mut screen = match (&locator, &key) {
            (Some(l), Some(k)) => locate(l, k).await,
            _ => None,
        };
        let place = |screen| {
            if question {
                ProbePlace::Question
            } else {
                ProbePlace::Monitor { screen }
            }
        };
        let show = |screen| ProbeEvent::Show {
            id,
            probe: probe.clone(),
            place: place(screen),
        };
        if self.events.send(show(screen)).is_err() {
            return ProbeOutcome::Failed(NO_UI.to_owned());
        }
        let deadline = tokio::time::sleep_until(deadline);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                answer = &mut rx => {
                    return match answer {
                        Ok(true) => ProbeOutcome::Seen,
                        Ok(false) | Err(_) => ProbeOutcome::Unseen,
                    };
                }
                () = &mut deadline => return ProbeOutcome::Unseen,
                () = tokio::time::sleep(LOCATE_EVERY), if locator.is_some() => {
                    if let (Some(l), Some(k)) = (&locator, &key) {
                        let now = locate(l, k).await;
                        if now != screen {
                            screen = now;
                            let _ = self.events.send(show(screen));
                        }
                    }
                }
            }
        }
    }
}

/// Busca el monitor fuera del runtime. Un error se registra y cuenta como no encontrado:
/// la UI tiene su propio respaldo (el nombre del EDID).
async fn locate(locator: &Arc<dyn ScreenLocator>, key: &EdidKey) -> Option<ProbeScreen> {
    let (locator, key) = (locator.clone(), key.clone());
    let found = tokio::task::spawn_blocking(move || locator.locate(&key)).await;
    match found {
        Ok(Ok(bounds)) => bounds.and_then(probe_screen),
        Ok(Err(e)) => {
            tracing::debug!("no se pudo buscar el monitor de la sonda: {e}");
            None
        }
        Err(e) => {
            tracing::warn!("la búsqueda del monitor de la sonda terminó mal: {e}");
            None
        }
    }
}

fn probe_screen(b: Bounds) -> Option<ProbeScreen> {
    Some(ProbeScreen {
        x: b.x,
        y: b.y,
        width: u32::try_from(b.width).ok().filter(|w| *w > 0)?,
        height: u32::try_from(b.height).ok().filter(|h| *h > 0)?,
    })
}

fn edid_key(m: &ProbeMonitor) -> Option<EdidKey> {
    let e = m.edid.as_ref()?;
    Some(EdidKey {
        manufacturer: e.manufacturer.clone(),
        product_code: e.product_code,
        serial_number: e.serial_number,
        name: Some(m.name.clone()),
    })
}

/// La sonda del motor tal como viaja a la otra máquina y a la UI.
pub fn wire_probe(probe: &Probe) -> CalibrationProbe {
    let edid = probe.monitor.edid.as_ref();
    CalibrationProbe {
        monitor: ProbeMonitor {
            edid: edid.map(|e| EdidId {
                manufacturer: e.manufacturer.clone(),
                product_code: e.product_code,
                serial_number: e.serial_number,
            }),
            name: edid
                .and_then(|e| e.name.clone())
                .unwrap_or_else(|| probe.monitor.description.clone()),
        },
        input: probe.input,
        ask: match probe.kind {
            ProbeKind::Candidate { code } => ProbeAsk::Candidate { code },
            ProbeKind::UserSwitch => ProbeAsk::UserSwitch,
        },
        timeout_ms: u64::try_from(probe.timeout.as_millis()).unwrap_or(u64::MAX),
    }
}

/// El [`Confirmer`] real: muestra la sonda en la máquina de la entrada que debería verse.
///
/// `home` es la entrada de esta máquina en el monitor que se calibra (en la PC, su
/// DisplayPort); la otra entrada es la de la otra máquina.
pub struct AgentConfirmer {
    agent: Arc<Agent>,
    home: Input,
}

impl AgentConfirmer {
    pub fn new(agent: Arc<Agent>, home: Input) -> Self {
        AgentConfirmer { agent, home }
    }
}

impl Confirmer for AgentConfirmer {
    fn confirm<'a>(&'a self, probe: &'a Probe) -> ConfirmFuture<'a> {
        Box::pin(async move {
            let wire = wire_probe(probe);
            let board = self.agent.probes();
            let outcome = if probe.input == self.home {
                board.show(&wire, false).await
            } else {
                let peer = self.agent.orchestrator().peer();
                match peer.show_probe(&wire).await {
                    Ok(ProbeOutcome::Failed(why)) | Err(why) => {
                        tracing::info!(
                            "sonda de calibración sin la otra máquina ({why}): se pregunta aquí"
                        );
                        board.show(&wire, true).await
                    }
                    Ok(outcome) => outcome,
                }
            };
            match outcome {
                ProbeOutcome::Seen => Ok(true),
                ProbeOutcome::Unseen => Ok(false),
                ProbeOutcome::Failed(why) => Err(ConfirmError(why)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use tandem_display::ddc::{Edid, MonitorInfo};

    use super::*;

    fn probe(timeout_ms: u64) -> CalibrationProbe {
        CalibrationProbe {
            monitor: ProbeMonitor {
                edid: Some(EdidId {
                    manufacturer: "SAM".into(),
                    product_code: 0x7489,
                    serial_number: 0,
                }),
                name: "Odyssey G5".into(),
            },
            input: Input::Hdmi,
            ask: ProbeAsk::Candidate { code: 0x05 },
            timeout_ms,
        }
    }

    async fn next(rx: &mut broadcast::Receiver<ProbeEvent>) -> ProbeEvent {
        rx.recv().await.unwrap()
    }

    fn shown_id(e: &ProbeEvent) -> u64 {
        match e {
            ProbeEvent::Show { id, .. } => *id,
            other => panic!("se esperaba Show: {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_click_in_time_is_seen_and_the_probe_goes_away() {
        let board = Arc::new(ProbeBoard::new(None));
        let mut ui = board.subscribe();
        let task = tokio::spawn({
            let board = board.clone();
            async move { board.show(&probe(8000), false).await }
        });
        let shown = next(&mut ui).await;
        assert!(matches!(
            &shown,
            ProbeEvent::Show {
                place: ProbePlace::Monitor { screen: None },
                ..
            }
        ));
        let id = shown_id(&shown);
        board.answer(id, true).unwrap();
        assert_eq!(task.await.unwrap(), ProbeOutcome::Seen);
        assert_eq!(next(&mut ui).await, ProbeEvent::Hide { id });
        // Ya contestada no se puede volver a contestar.
        assert_eq!(board.answer(id, true), Err(UnknownProbe(id)));
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_click_it_counts_as_unseen_and_disappears() {
        let board = Arc::new(ProbeBoard::new(None));
        let mut ui = board.subscribe();
        let start = tokio::time::Instant::now();
        let task = tokio::spawn({
            let board = board.clone();
            async move { board.show(&probe(8000), false).await }
        });
        let id = shown_id(&next(&mut ui).await);
        assert_eq!(task.await.unwrap(), ProbeOutcome::Unseen);
        assert_eq!(start.elapsed(), Duration::from_millis(8000));
        assert_eq!(next(&mut ui).await, ProbeEvent::Hide { id });
        // Un clic tardío no tiene a quién llegar.
        assert_eq!(board.answer(id, true), Err(UnknownProbe(id)));
    }

    #[tokio::test(start_paused = true)]
    async fn no_and_dropping_the_wait_both_hide_the_probe() {
        let board = Arc::new(ProbeBoard::new(None));
        let mut ui = board.subscribe();
        let task = tokio::spawn({
            let board = board.clone();
            async move { board.show(&probe(8000), true).await }
        });
        let shown = next(&mut ui).await;
        assert!(matches!(
            shown,
            ProbeEvent::Show {
                place: ProbePlace::Question,
                ..
            }
        ));
        board.answer(shown_id(&shown), false).unwrap();
        assert_eq!(task.await.unwrap(), ProbeOutcome::Unseen);
        assert!(matches!(next(&mut ui).await, ProbeEvent::Hide { .. }));

        // Quien calibra deja de esperar (canceló): la sonda se quita igual.
        let task = tokio::spawn({
            let board = board.clone();
            async move { board.show(&probe(8000), false).await }
        });
        let id = shown_id(&next(&mut ui).await);
        task.abort();
        assert_eq!(next(&mut ui).await, ProbeEvent::Hide { id });
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_window_it_fails_at_once() {
        let board = ProbeBoard::new(None);
        assert_eq!(
            board.show(&probe(8000), false).await,
            ProbeOutcome::Failed(NO_UI.into())
        );
    }

    /// El monitor no está en el escritorio hasta la tercera búsqueda (el sistema tarda
    /// en notar el cambio de entrada).
    struct Appears {
        calls: AtomicUsize,
        keys: Mutex<Vec<EdidKey>>,
    }

    impl ScreenLocator for Appears {
        fn locate(&self, key: &EdidKey) -> Result<Option<Bounds>, String> {
            self.keys.lock().unwrap().push(key.clone());
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((n >= 2).then_some(Bounds {
                x: -2560,
                y: 0,
                width: 2560,
                height: 1440,
            }))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_probe_follows_the_monitor_when_it_appears() {
        let locator = Arc::new(Appears {
            calls: AtomicUsize::new(0),
            keys: Mutex::new(Vec::new()),
        });
        let board = Arc::new(ProbeBoard::new(Some(locator.clone())));
        let mut ui = board.subscribe();
        let task = tokio::spawn({
            let board = board.clone();
            async move { board.show(&probe(8000), false).await }
        });
        let first = next(&mut ui).await;
        let id = shown_id(&first);
        assert!(matches!(
            first,
            ProbeEvent::Show {
                place: ProbePlace::Monitor { screen: None },
                ..
            }
        ));
        // La segunda búsqueda tampoco lo ve: no se repite el aviso. La tercera sí.
        let moved = next(&mut ui).await;
        assert_eq!(
            moved,
            ProbeEvent::Show {
                id,
                probe: probe(8000),
                place: ProbePlace::Monitor {
                    screen: Some(ProbeScreen {
                        x: -2560,
                        y: 0,
                        width: 2560,
                        height: 1440,
                    }),
                },
            }
        );
        assert_eq!(locator.calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            locator.keys.lock().unwrap()[0],
            EdidKey {
                manufacturer: "SAM".into(),
                product_code: 0x7489,
                serial_number: 0,
                name: Some("Odyssey G5".into()),
            }
        );
        board.answer(id, true).unwrap();
        assert_eq!(task.await.unwrap(), ProbeOutcome::Seen);
    }

    #[tokio::test(start_paused = true)]
    async fn a_question_does_not_look_for_the_monitor() {
        let locator = Arc::new(Appears {
            calls: AtomicUsize::new(0),
            keys: Mutex::new(Vec::new()),
        });
        let board = Arc::new(ProbeBoard::new(Some(locator.clone())));
        let mut ui = board.subscribe();
        let task = tokio::spawn({
            let board = board.clone();
            async move { board.show(&probe(2000), true).await }
        });
        next(&mut ui).await;
        assert_eq!(task.await.unwrap(), ProbeOutcome::Unseen);
        assert_eq!(locator.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn engine_probes_travel_with_their_edid() {
        let monitor = MonitorInfo {
            index: 0,
            description: "Monitor PnP genérico".into(),
            device: r"\\.\DISPLAY1".into(),
            edid: Some(Edid {
                manufacturer: "SAM".into(),
                product_code: 0x711A,
                serial_number: 7,
                name: Some("LC34G55T".into()),
                serial_text: None,
            }),
        };
        let p = Probe {
            monitor: monitor.clone(),
            input: Input::DisplayPort,
            kind: ProbeKind::Candidate { code: 0x09 },
            timeout: Duration::from_secs(8),
        };
        assert_eq!(
            wire_probe(&p),
            CalibrationProbe {
                monitor: ProbeMonitor {
                    edid: Some(EdidId {
                        manufacturer: "SAM".into(),
                        product_code: 0x711A,
                        serial_number: 7,
                    }),
                    name: "LC34G55T".into(),
                },
                input: Input::DisplayPort,
                ask: ProbeAsk::Candidate { code: 0x09 },
                timeout_ms: 8000,
            }
        );
        // Sin EDID solo queda la descripción del sistema.
        let bare = Probe {
            monitor: MonitorInfo {
                edid: None,
                ..monitor
            },
            kind: ProbeKind::UserSwitch,
            timeout: Duration::from_secs(120),
            ..p
        };
        let w = wire_probe(&bare);
        assert_eq!(
            w.monitor,
            ProbeMonitor {
                edid: None,
                name: "Monitor PnP genérico".into()
            }
        );
        assert_eq!((w.ask, w.timeout_ms), (ProbeAsk::UserSwitch, 120_000));
        assert_eq!(edid_key(&w.monitor), None);
    }

    #[test]
    fn empty_bounds_are_not_a_screen() {
        let b = |width, height| Bounds {
            x: 0,
            y: 0,
            width,
            height,
        };
        assert!(probe_screen(b(10, 10)).is_some());
        assert_eq!(probe_screen(b(0, 10)), None);
        assert_eq!(probe_screen(b(10, -1)), None);
    }
}
