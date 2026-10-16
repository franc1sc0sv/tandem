//! Los pasos de pantalla de esta máquina (`SwitchInput`, `Attach`, `Detach`,
//! `SetBuiltin`), delegados en los backends de `tandem-display`.
//!
//! Las entradas las cambia la PC (DESIGN §8): el orquestador ya le manda a ella los
//! `SwitchInput` pedidos en la Mac, así que la Mac no tiene [`MonitorSwitcher`]. Lo que
//! todavía no tiene backend responde «no implementado» y dispara la vuelta atrás.

use std::sync::Arc;

use tandem_core::{BuiltinDisplay, Config, Machine, MonitorConfig, MonitorId, Step};
use tandem_display::desktop::{self, DesktopControl, DesktopError};
use tandem_display::switcher::{
    DdcSwitcher, ManualEvent, ManualSwitcher, MonitorSwitcher, PerMonitor, SwitchRequest,
};
use tandem_proto::ManualSwitchEvent;
use tokio::sync::watch;

use crate::add_monitor::LiveDesk;
use crate::config::AgentConfig;
use crate::input::GeometrySteps;
use crate::orchestrator::{LocalSteps, STEP_NOT_IMPLEMENTED, StepFuture, Unimplemented, describe};
use crate::power::PowerSteps;

/// Ejecutor local de los pasos de pantalla; el resto va a `others`.
pub struct DisplaySteps {
    me: Machine,
    desk: Config,
    switcher: Option<Arc<dyn MonitorSwitcher>>,
    desktop: Option<Arc<dyn DesktopControl>>,
    /// Turno del escritorio: un paso que venció sigue corriendo en su hilo
    /// bloqueante, y el siguiente (su inverso) tiene que esperar a que termine.
    desktop_turn: Arc<tokio::sync::Mutex<()>>,
    others: Arc<dyn LocalSteps>,
    /// Los códigos DDC aprendidos al calibrar (T-080), por encima de los de `desk`.
    live: LiveDesk,
}

impl std::fmt::Debug for DisplaySteps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplaySteps")
            .field("me", &self.me)
            .field("switcher", &self.switcher.is_some())
            .field("desktop", &self.desktop.is_some())
            .finish_non_exhaustive()
    }
}

impl DisplaySteps {
    /// Sin backends: todo responde «no implementado».
    pub fn new(me: Machine, desk: Config) -> Self {
        DisplaySteps {
            me,
            desk,
            switcher: None,
            desktop: None,
            desktop_turn: Arc::default(),
            others: Arc::new(Unimplemented),
            live: LiveDesk::default(),
        }
    }

    /// Lo que «Agregar monitor» aprende: el siguiente cambio de modo ya lo usa.
    pub fn with_live(mut self, live: LiveDesk) -> Self {
        self.live = live;
        self
    }

    pub fn with_switcher(mut self, switcher: Arc<dyn MonitorSwitcher>) -> Self {
        self.switcher = Some(switcher);
        self
    }

    pub fn with_desktop(mut self, desktop: Arc<dyn DesktopControl>) -> Self {
        self.desktop = Some(desktop);
        self
    }

    /// Los pasos que no son de pantalla (energía, geometría del teclado y mouse).
    pub fn with_others(mut self, others: Arc<dyn LocalSteps>) -> Self {
        self.others = others;
        self
    }

    /// Los backends reales de esta máquina. En la PC: DDC para los monitores con perfil
    /// y el cambio manual por la UI para los demás. El escritorio es la topología del
    /// sistema (Windows en la PC, Quartz en la Mac), si el agente corre en el suyo. Los
    /// pasos de energía van a [`PowerSteps`].
    /// `UpdateInputGeometry` sube `geometry`, para que el motor de entrada vuelva a leer
    /// las pantallas ([`GeometrySteps`]).
    pub fn platform(
        cfg: &AgentConfig,
        manual: Arc<ManualSwitcher>,
        geometry: Arc<watch::Sender<u64>>,
    ) -> Self {
        let mut steps = DisplaySteps::new(cfg.machine, cfg.desk.clone()).with_others(Arc::new(
            PowerSteps::for_machine(cfg, Arc::new(GeometrySteps::new(geometry))),
        ));
        if cfg.machine == Machine::Pc {
            steps = steps.with_switcher(Arc::new(PerMonitor {
                ddc: Arc::new(ddc_backend()),
                manual,
            }));
        }
        if let Some(desktop) = desktop::native(cfg.machine) {
            steps = steps.with_desktop(desktop);
        }
        steps
    }

    fn monitor(&self, id: &MonitorId) -> Result<MonitorConfig, String> {
        let mut m = self
            .desk
            .monitor(id)
            .cloned()
            .ok_or_else(|| format!("el monitor `{id}` no está en la configuración"))?;
        self.live.apply(&mut m);
        Ok(m)
    }

    async fn switch_input(&self, monitor: &MonitorId, to: Machine) -> Result<(), String> {
        if self.me != Machine::Pc {
            return Err("las entradas de los monitores las cambia la PC".to_owned());
        }
        let switcher = self.switcher.as_ref().ok_or(STEP_NOT_IMPLEMENTED)?;
        let monitor = self.monitor(monitor)?;
        let input = monitor
            .input_for(to)
            .ok_or_else(|| format!("{} no está cableado a {to}", monitor.id))?;
        let req = SwitchRequest { monitor, to, input };
        switcher.switch(&req).await.map_err(|e| e.to_string())
    }

    async fn on_desktop(
        &self,
        machine: Machine,
        op: impl FnOnce(&dyn DesktopControl) -> Result<(), DesktopError> + Send + 'static,
    ) -> Result<(), String> {
        if machine != self.me {
            return Err(format!("no le corresponde a {}", self.me));
        }
        let desktop = self.desktop.clone().ok_or(STEP_NOT_IMPLEMENTED)?;
        let turn = self.desktop_turn.clone().lock_owned().await;
        // Las APIs de topología bloquean (SetDisplayConfig tarda cientos de ms).
        tokio::task::spawn_blocking(move || {
            let _turn = turn;
            op(desktop.as_ref())
        })
        .await
        .map_err(|e| format!("el backend de pantallas terminó mal: {e}"))?
        .map_err(|e| e.to_string())
    }
}

impl LocalSteps for DisplaySteps {
    fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
        Box::pin(async move {
            let r = match step {
                Step::SwitchInput { monitor, to, .. } => self.switch_input(monitor, *to).await,
                Step::Attach { machine, monitor } => match self.monitor(monitor) {
                    Ok(m) => self.on_desktop(*machine, move |d| d.attach(&m)).await,
                    Err(e) => Err(e),
                },
                Step::Detach { machine, monitor } => match self.monitor(monitor) {
                    Ok(m) => self.on_desktop(*machine, move |d| d.detach(&m)).await,
                    Err(e) => Err(e),
                },
                Step::SetBuiltin { to, .. } => {
                    let to: BuiltinDisplay = *to;
                    self.on_desktop(Machine::Mac, move |d| d.set_builtin(to))
                        .await
                }
                Step::PowerOnPc | Step::PowerOffPc | Step::UpdateInputGeometry => {
                    return self.others.execute(step).await;
                }
            };
            r.map_err(|e| format!("{}: {e}", describe(step)))
        })
    }
}

#[cfg(windows)]
fn ddc_backend() -> DdcSwitcher {
    DdcSwitcher::windows()
}

/// Fuera de Windows no hay DDC todavía (`ddc-macos` depende del spike de la sección 11).
#[cfg(not(windows))]
fn ddc_backend() -> DdcSwitcher {
    DdcSwitcher::new(Arc::new(|| Err(tandem_display::ddc::DdcError::Unsupported)))
}

/// El aviso del backend manual tal como viaja a la UI.
pub fn manual_event_to_ui(ev: ManualEvent) -> ManualSwitchEvent {
    match ev {
        ManualEvent::Requested(p) => ManualSwitchEvent::Requested {
            id: p.id,
            monitor: p.monitor,
            label: p.label,
            to: p.to,
            input: p.input,
        },
        ManualEvent::Closed { id } => ManualSwitchEvent::Closed { id },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, PoisonError, mpsc};

    use tandem_core::{Input, Mode, State};
    use tandem_display::switcher::{SwitchError, SwitchFuture};
    use tandem_proto::{Reply, UiCommand};

    use super::*;
    use crate::agent::Agent;
    use crate::agent::tests::until_outcome;
    use crate::config::fixtures;
    use crate::orchestrator::tests::{FakePeer, FakeSteps};
    use crate::orchestrator::{Orchestrator, PeerLink, StepTimeouts};
    use crate::store::StateStore;

    /// Cambios de entrada simulados: registran (monitor, máquina, entrada).
    #[derive(Default)]
    struct Switches {
        log: Mutex<Vec<(MonitorId, Machine, Input)>>,
        fail: bool,
    }

    impl MonitorSwitcher for Switches {
        fn switch<'a>(&'a self, req: &'a SwitchRequest) -> SwitchFuture<'a> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((req.monitor.id.clone(), req.to, req.input));
                if self.fail {
                    Err(SwitchError::Declined(req.monitor.id.clone()))
                } else {
                    Ok(())
                }
            })
        }
    }

    impl Switches {
        fn seen(&self) -> Vec<(MonitorId, Machine, Input)> {
            self.log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// Escritorio simulado: registra cada llamada como texto.
    #[derive(Default)]
    struct Desktop(Mutex<Vec<String>>);

    impl DesktopControl for Desktop {
        fn attach(&self, monitor: &MonitorConfig) -> Result<(), DesktopError> {
            self.push(format!("attach {}", monitor.id))
        }
        fn detach(&self, monitor: &MonitorConfig) -> Result<(), DesktopError> {
            self.push(format!("detach {}", monitor.id))
        }
        fn set_builtin(&self, mode: BuiltinDisplay) -> Result<(), DesktopError> {
            if mode == BuiltinDisplay::Mirror {
                return Err(DesktopError("espejo no disponible".into()));
            }
            self.push(format!("builtin {mode:?}"))
        }
    }

    impl Desktop {
        fn push(&self, s: String) -> Result<(), DesktopError> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(s);
            Ok(())
        }
        fn calls(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// El otro agente sin red: sus pasos locales, tal cual.
    struct Direct(Arc<dyn LocalSteps>);

    impl PeerLink for Direct {
        fn execute<'a>(&'a self, step: &'a Step) -> StepFuture<'a> {
            self.0.execute(step)
        }
        fn publish_state(&self, _state: &State) {}
    }

    fn desk() -> Config {
        fixtures::desk().desk
    }

    fn switch(monitor: &str, to: Machine) -> Step {
        Step::SwitchInput {
            monitor: monitor.into(),
            from: to.other(),
            to,
            input: desk()
                .monitor(&monitor.into())
                .unwrap()
                .input_for(to)
                .unwrap(),
        }
    }

    #[tokio::test]
    async fn on_the_mac_switch_input_goes_to_the_pc() {
        let (mac_sw, pc_sw) = (Arc::new(Switches::default()), Arc::new(Switches::default()));
        let mac = DisplaySteps::new(Machine::Mac, desk()).with_switcher(mac_sw.clone());
        let pc = DisplaySteps::new(Machine::Pc, desk()).with_switcher(pc_sw.clone());
        let o = Orchestrator::new(Machine::Mac, Arc::new(mac), StepTimeouts::default());
        o.set_peer(Arc::new(Direct(Arc::new(pc))));

        o.dispatch(&switch("m34", Machine::Pc)).await.unwrap();
        o.dispatch(&switch("m32", Machine::Mac)).await.unwrap();
        assert_eq!(
            pc_sw.seen(),
            [
                ("m34".into(), Machine::Pc, Input::DisplayPort),
                ("m32".into(), Machine::Mac, Input::Hdmi),
            ]
        );
        assert!(mac_sw.seen().is_empty(), "la Mac no cambia entradas");
    }

    #[tokio::test]
    async fn the_mac_never_switches_inputs_itself() {
        let sw = Arc::new(Switches::default());
        let mac = DisplaySteps::new(Machine::Mac, desk()).with_switcher(sw.clone());
        let e = mac.execute(&switch("m34", Machine::Pc)).await.unwrap_err();
        assert_eq!(
            e,
            "cambiar la entrada de m34 a pc: las entradas de los monitores las cambia la PC"
        );
        assert!(sw.seen().is_empty());
    }

    #[tokio::test]
    async fn the_input_comes_from_the_wiring() {
        let sw = Arc::new(Switches::default());
        let pc = DisplaySteps::new(Machine::Pc, desk()).with_switcher(sw.clone());
        // El paso trae una entrada; manda la del cableado de la configuración.
        let step = Step::SwitchInput {
            monitor: "m34".into(),
            from: Machine::Pc,
            to: Machine::Mac,
            input: Input::DisplayPort,
        };
        pc.execute(&step).await.unwrap();
        assert_eq!(sw.seen(), [("m34".into(), Machine::Mac, Input::Hdmi)]);
    }

    #[tokio::test]
    async fn switch_errors_are_reported_with_the_step() {
        let sw = Arc::new(Switches {
            fail: true,
            ..Default::default()
        });
        let pc = DisplaySteps::new(Machine::Pc, desk()).with_switcher(sw);
        let e = pc.execute(&switch("m32", Machine::Pc)).await.unwrap_err();
        assert_eq!(
            e,
            "cambiar la entrada de m32 a pc: m32: el usuario no confirmó el cambio manual"
        );
        let unknown = Step::SwitchInput {
            monitor: "m99".into(),
            from: Machine::Mac,
            to: Machine::Pc,
            input: Input::DisplayPort,
        };
        let e = pc.execute(&unknown).await.unwrap_err();
        assert!(
            e.ends_with("el monitor `m99` no está en la configuración"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn steps_without_backend_are_not_implemented() {
        let pc = DisplaySteps::new(Machine::Pc, desk());
        let mac = DisplaySteps::new(Machine::Mac, desk());
        for (steps, step) in [
            (&pc, switch("m34", Machine::Pc)),
            (
                &pc,
                Step::Attach {
                    machine: Machine::Pc,
                    monitor: "m34".into(),
                },
            ),
            (
                &mac,
                Step::Detach {
                    machine: Machine::Mac,
                    monitor: "m34".into(),
                },
            ),
            (
                &mac,
                Step::SetBuiltin {
                    from: BuiltinDisplay::Off,
                    to: BuiltinDisplay::Extend,
                },
            ),
            (&mac, Step::PowerOnPc),
            (&pc, Step::PowerOffPc),
            (&pc, Step::UpdateInputGeometry),
        ] {
            let e = steps.execute(&step).await.unwrap_err();
            assert_eq!(e, format!("{}: {STEP_NOT_IMPLEMENTED}", describe(&step)));
        }
    }

    #[tokio::test]
    async fn desktop_steps_go_to_the_backend_of_their_machine() {
        let d = Arc::new(Desktop::default());
        let mac = DisplaySteps::new(Machine::Mac, desk()).with_desktop(d.clone());
        mac.execute(&Step::Detach {
            machine: Machine::Mac,
            monitor: "m34".into(),
        })
        .await
        .unwrap();
        mac.execute(&Step::Attach {
            machine: Machine::Mac,
            monitor: "m32".into(),
        })
        .await
        .unwrap();
        mac.execute(&Step::SetBuiltin {
            from: BuiltinDisplay::Off,
            to: BuiltinDisplay::Extend,
        })
        .await
        .unwrap();
        // El error del backend llega con el paso.
        let e = mac
            .execute(&Step::SetBuiltin {
                from: BuiltinDisplay::Off,
                to: BuiltinDisplay::Mirror,
            })
            .await
            .unwrap_err();
        assert_eq!(e, "pantalla integrada en espejo: espejo no disponible");
        // Lo que le toca a la otra máquina no se hace aquí.
        let e = mac
            .execute(&Step::Attach {
                machine: Machine::Pc,
                monitor: "m34".into(),
            })
            .await
            .unwrap_err();
        assert!(e.ends_with("no le corresponde a mac"), "{e}");
        assert_eq!(d.calls(), ["detach m34", "attach m32", "builtin Extend"]);

        let pc = DisplaySteps::new(Machine::Pc, desk()).with_desktop(d.clone());
        assert!(
            pc.execute(&Step::SetBuiltin {
                from: BuiltinDisplay::Off,
                to: BuiltinDisplay::Extend,
            })
            .await
            .is_err()
        );
        assert_eq!(d.calls().len(), 3);
    }

    #[tokio::test]
    async fn other_steps_are_delegated() {
        let others = Arc::new(FakeSteps::default());
        let mac = DisplaySteps::new(Machine::Mac, desk()).with_others(others.clone());
        mac.execute(&Step::PowerOnPc).await.unwrap();
        mac.execute(&Step::UpdateInputGeometry).await.unwrap();
        assert_eq!(others.steps(), [Step::PowerOnPc, Step::UpdateInputGeometry]);
    }

    #[tokio::test]
    async fn platform_backends_by_machine() {
        let mut cfg = fixtures::desk();
        let manual = Arc::new(ManualSwitcher::new());
        // La Mac no tiene cómo cambiar entradas.
        let geometry = Arc::new(watch::Sender::new(0));
        let mac = DisplaySteps::platform(&cfg, manual.clone(), geometry.clone());
        assert!(mac.switcher.is_none());
        cfg.machine = Machine::Pc;
        let pc = DisplaySteps::platform(&cfg, manual.clone(), geometry);
        // El escritorio de cada máquina solo existe en su sistema.
        assert_eq!(mac.desktop.is_some(), cfg!(target_os = "macos"));
        assert_eq!(pc.desktop.is_some(), cfg!(windows));
        // El 32" no tiene perfil DDC: se pide a mano, y sin UI falla enseguida.
        let e = pc.execute(&switch("m32", Machine::Pc)).await.unwrap_err();
        assert!(
            e.ends_with("no hay una UI conectada para pedir el cambio manual"),
            "{e}"
        );
        // El 34" va por DDC.
        let e = pc.execute(&switch("m34", Machine::Pc)).await.unwrap_err();
        if cfg!(windows) {
            // En el runner no hay monitores con DDC.
            assert!(!e.contains("UI"), "{e}");
        } else {
            assert!(
                e.ends_with("DDC/CI no está disponible en esta plataforma"),
                "{e}"
            );
        }
    }

    #[tokio::test]
    async fn platform_routes_power_steps_to_power_backends() {
        // Sin [power.wol] la Mac no puede despertar a la PC, y lo dice.
        let cfg = fixtures::desk();
        let geometry = Arc::new(watch::Sender::new(0));
        let mut rx = geometry.subscribe();
        let mac = DisplaySteps::platform(&cfg, Arc::new(ManualSwitcher::new()), geometry);
        let e = mac.execute(&Step::PowerOnPc).await.unwrap_err();
        assert!(e.contains(crate::power::WOL_NOT_CONFIGURED), "{e}");
        // La geometría la recalcula el motor de entrada: el paso solo le avisa.
        mac.execute(&Step::UpdateInputGeometry).await.unwrap();
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), 1);
    }

    /// La PC orquesta Solo Windows: el 34" cambia por DDC (simulado) y el 32" lo cambia el
    /// usuario a mano, confirmando en la UI de la PC.
    #[tokio::test]
    async fn manual_switch_waits_for_the_ui_inside_a_mode_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = fixtures::desk();
        cfg.machine = Machine::Pc;
        let manual = Arc::new(ManualSwitcher::new());
        let ddc = Arc::new(Switches::default());
        let local = DisplaySteps::new(Machine::Pc, cfg.desk.clone())
            .with_switcher(Arc::new(PerMonitor {
                ddc: ddc.clone(),
                manual: manual.clone(),
            }))
            .with_desktop(Arc::new(Desktop::default()))
            .with_others(Arc::new(FakeSteps::default()));
        let a = Arc::new(Agent::with_backends(
            cfg,
            StateStore::new(dir.path().join("state.json")),
            Arc::new(local),
            manual,
        ));
        a.set_peer(Arc::new(FakePeer(Arc::new(FakeSteps::default()))));
        let mut s = a.state();
        s.pc_on = true;
        a.set_state(s).unwrap();

        let mut ui = a.subscribe_manual();
        let mut txns = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetMode(Mode::SoloPc)),
            Reply::Accepted { .. }
        ));
        let ManualEvent::Requested(p) = ui.recv().await.unwrap() else {
            panic!("se esperaba un pedido");
        };
        assert_eq!(
            (p.monitor.0.as_str(), p.to, p.input),
            ("m32", Machine::Pc, Input::DisplayPort)
        );
        assert_eq!(
            ddc.seen(),
            [("m34".into(), Machine::Pc, Input::DisplayPort)]
        );
        // Mientras el usuario no conteste, la transacción no termina.
        tokio::task::yield_now().await;
        let so_far: Vec<_> = std::iter::from_fn(|| txns.try_recv().ok()).collect();
        assert!(
            !so_far.is_empty() && !so_far.iter().any(is_outcome),
            "{so_far:?}"
        );

        assert_eq!(
            a.handle(UiCommand::AnswerManualSwitch {
                id: p.id,
                done: true
            }),
            Reply::Done
        );
        let events = until_outcome(&mut txns).await;
        assert!(
            matches!(
                events.last(),
                Some(tandem_proto::TxnEvent::Committed { .. })
            ),
            "{events:?}"
        );
        assert!(a.state().owner.values().all(|m| *m == Machine::Pc));
        assert_eq!(ui.recv().await.unwrap(), ManualEvent::Closed { id: p.id });
        // Una respuesta a un pedido que ya no existe se rechaza.
        assert!(matches!(
            a.handle(UiCommand::AnswerManualSwitch {
                id: p.id,
                done: true
            }),
            Reply::Rejected(_)
        ));
    }

    fn is_outcome(e: &tandem_proto::TxnEvent) -> bool {
        use tandem_proto::TxnEvent::*;
        matches!(
            e,
            Committed { .. } | RolledBack { .. } | Inconsistent { .. }
        )
    }

    #[test]
    fn manual_events_keep_their_fields_for_the_ui() {
        use tandem_display::switcher::ManualPrompt;
        let ev = manual_event_to_ui(ManualEvent::Requested(ManualPrompt {
            id: 3,
            monitor: "m32".into(),
            label: "32\"".into(),
            to: Machine::Mac,
            input: Input::Hdmi,
        }));
        assert_eq!(
            ev,
            ManualSwitchEvent::Requested {
                id: 3,
                monitor: "m32".into(),
                label: "32\"".into(),
                to: Machine::Mac,
                input: Input::Hdmi,
            }
        );
        assert_eq!(
            manual_event_to_ui(ManualEvent::Closed { id: 3 }),
            ManualSwitchEvent::Closed { id: 3 }
        );
    }

    /// Aviso de que empezó, permiso para seguir y aviso de que terminó.
    type Gate = (mpsc::Sender<()>, mpsc::Receiver<()>, mpsc::Sender<()>);

    /// Un escritorio cuyo primer `attach` se queda bloqueado hasta que la prueba lo suelta.
    struct GatedDesktop {
        inner: Desktop,
        gate: Mutex<Option<Gate>>,
    }

    impl DesktopControl for GatedDesktop {
        fn attach(&self, monitor: &MonitorConfig) -> Result<(), DesktopError> {
            let gate = self.gate.lock().unwrap().take();
            if let Some((started, release, _)) = &gate {
                started.send(()).unwrap();
                release.recv().unwrap();
            }
            let r = self.inner.attach(monitor);
            if let Some((_, _, done)) = gate {
                done.send(()).unwrap();
            }
            r
        }
        fn detach(&self, monitor: &MonitorConfig) -> Result<(), DesktopError> {
            self.inner.detach(monitor)
        }
        fn set_builtin(&self, mode: BuiltinDisplay) -> Result<(), DesktopError> {
            self.inner.set_builtin(mode)
        }
    }

    // QA ronda 2: si agregar un monitor vencía, su hilo bloqueante seguía y el
    // inverso (quitarlo) podía correr antes: el monitor quedaba agregado, fantasma.
    #[tokio::test]
    async fn a_timed_out_desktop_step_cannot_land_after_its_inverse() {
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let desktop = Arc::new(GatedDesktop {
            inner: Desktop::default(),
            gate: Mutex::new(Some((started_tx, release_rx, done_tx))),
        });
        let pc = Arc::new(DisplaySteps::new(Machine::Pc, desk()).with_desktop(desktop.clone()));
        let attach = Step::Attach {
            machine: Machine::Pc,
            monitor: "m34".into(),
        };
        {
            let mut first = pc.execute(&attach);
            tokio::select! {
                biased;
                _ = &mut first => panic!("el paso colgado terminó"),
                _ = tokio::task::spawn_blocking(move || started.recv()) => {}
            }
        }
        let undo = tokio::spawn({
            let (pc, inv) = (pc.clone(), attach.inverse(&desk()).unwrap());
            async move { pc.execute(&inv).await }
        });
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        release.send(()).unwrap();
        undo.await.unwrap().unwrap();
        tokio::task::spawn_blocking(move || done.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(desktop.inner.calls(), ["attach m34", "detach m34"]);
    }
}
