//! Lo que el agente hace para el asistente de configuración inicial (T-087): detectar lo
//! conectado, agregar y quitar monitores del puesto, restablecerlo, la prueba «presiona
//! una tecla» y dónde está el teclado.
//!
//! Ninguna de estas operaciones quita una salida de ningún escritorio: solo el paso
//! `Detach` de un cambio de modo lo hace ([`crate::safety`]).

use std::sync::{Arc, PoisonError};

use tandem_core::{Config, Machine, MonitorConfig, MonitorId};
use tandem_display::ddc::profiles::{ProfileDb, ProfileSource, user_dir};
use tandem_display::desktop::DesktopControl;
use tandem_proto::{ControlMsg, KeyProbeMsg, KeyProbeStatus, NewMonitor, Reply};
use tokio::sync::watch;

use super::{Agent, BUSY};
use crate::capabilities;
use crate::store::reconcile;
use crate::wizard::{self, MonitorScanner};

/// El escritorio de esta máquina, con un `Debug` para que el agente lo tenga.
#[derive(Clone)]
pub(super) struct DesktopHandle(pub Arc<dyn DesktopControl>);

impl std::fmt::Debug for DesktopHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DesktopHandle")
    }
}

impl Agent {
    /// Quien detecta de nuevo los monitores conectados (en el binario, el del sistema).
    pub fn set_scanner(&self, scanner: Arc<dyn MonitorScanner>) {
        *self.scanner.lock().unwrap_or_else(PoisonError::into_inner) = Some(scanner);
    }

    /// El directorio de datos, de donde salen los perfiles DDC del usuario.
    pub fn set_data_dir(&self, data_dir: &std::path::Path) {
        *self
            .user_profiles
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(user_dir(data_dir));
    }

    fn profile_db(&self) -> ProfileDb {
        let dir = self
            .user_profiles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        ProfileDb::load(dir.as_deref())
    }

    /// El puesto sin los monitores que ninguna máquina tiene conectados ahora.
    pub fn plan_desk(&self) -> Config {
        let caps = self.caps.borrow();
        capabilities::connected_desk(&self.desk(), &caps.here, caps.peer.as_ref())
    }

    /// Detecta de nuevo lo conectado a esta máquina y le pide lo mismo a la otra; con lo que
    /// ya se sabe de las dos arma los monitores físicos. Bloquea: fuera del runtime.
    pub(super) fn scan_monitors(&self) -> Reply {
        let scanner = self
            .scanner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(scanner) = scanner {
            match scanner.scan() {
                Ok(seen) => self.set_seen_monitors(seen),
                Err(e) => {
                    return Reply::Rejected(format!("no se pudieron detectar los monitores: {e}"));
                }
            }
            // La otra detecta lo suyo y lo anuncia: la UI vuelve a preguntar en un momento.
            self.orch.peer().send(ControlMsg::Rescan);
        }
        let caps = self.capabilities();
        Reply::Scan(wizard::propose(
            &caps.here,
            caps.peer.as_ref(),
            &self.desk(),
            &self.profile_db(),
        ))
    }

    /// La otra máquina pidió detectar de nuevo (su asistente está en el paso de monitores).
    pub fn rescan(&self) {
        let scanner = self
            .scanner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(Ok(seen)) = scanner.map(|s| s.scan()) {
            self.set_seen_monitors(seen);
        }
    }

    /// Agrega los monitores que el usuario eligió en el asistente.
    pub(super) fn add_monitors(&self, new: &[NewMonitor]) -> Reply {
        if new.is_empty() {
            return Reply::Done;
        }
        if self.orch.try_begin().is_none() {
            return Reply::Rejected(BUSY.into());
        }
        let mut desk = self.desk();
        let reply = match wizard::new_monitors(new, &desk, &self.profile_db()) {
            Ok(added) => {
                tracing::info!(
                    monitores = ?added.iter().map(|m| m.id.to_string()).collect::<Vec<_>>(),
                    "monitores agregados desde el asistente"
                );
                desk.monitors.extend(added);
                match desk.validate() {
                    Ok(()) => {
                        self.commit_desk(desk, &[]);
                        Reply::Done
                    }
                    Err(e) => Reply::Rejected(e.to_string()),
                }
            }
            Err(e) => Reply::Rejected(e),
        };
        self.orch.end();
        reply
    }

    /// Quita `monitor` del puesto y todo lo suyo: su entrada en `tandem.toml`, sus alias
    /// (vínculo entre identidades), su cableado, su perfil del usuario y su estado. La otra
    /// máquina lo hace al recibir el puesto nuevo.
    pub(super) fn remove_monitor(&self, monitor: &MonitorId) -> Reply {
        if self.orch.try_begin().is_none() {
            return Reply::Rejected(BUSY.into());
        }
        let mut desk = self.desk();
        let reply = match desk.monitor(monitor).cloned() {
            None => Reply::Rejected(format!("el monitor `{monitor}` no está en el puesto")),
            Some(gone) => {
                desk.remove_monitor(monitor);
                tracing::info!(%monitor, "monitor quitado del puesto");
                self.commit_desk(desk, &[gone]);
                Reply::Done
            }
        };
        self.orch.end();
        reply
    }

    /// «Restablecer puesto»: sin monitores, vínculos, perfiles del usuario de esos
    /// monitores ni estado, en las dos máquinas.
    pub(super) fn reset_desk(&self) -> Reply {
        if self.orch.try_begin().is_none() {
            return Reply::Rejected(BUSY.into());
        }
        let desk = self.desk();
        let gone = desk.monitors.clone();
        tracing::info!(monitores = gone.len(), "puesto restablecido");
        self.commit_desk(Config { monitors: vec![] }, &gone);
        self.orch.end();
        Reply::Done
    }

    /// El monitor no respondió a DDC: pierde sus códigos y queda en modo manual asistido
    /// (cada cambio de entrada se lo pide al usuario).
    pub(super) fn mark_manual(&self, monitor: &MonitorId) -> Reply {
        if self.orch.try_begin().is_none() {
            return Reply::Rejected(BUSY.into());
        }
        let mut desk = self.desk();
        let reply = match desk.monitors.iter_mut().find(|m| &m.id == monitor) {
            None => Reply::Rejected(format!("el monitor `{monitor}` no está en el puesto")),
            Some(m) => {
                if m.ddc.take().is_some() {
                    self.commit_desk(desk, &[]);
                }
                Reply::Done
            }
        };
        self.orch.end();
        reply
    }

    /// El puesto cambió aquí: queda vivo, el estado se ajusta, lo borrado se limpia y se
    /// guarda y avisa a la UI y a la otra máquina.
    fn commit_desk(&self, desk: Config, removed: &[MonitorConfig]) {
        // Antes de borrar nada: lo que esta máquina había quitado del escritorio vuelve.
        self.forget_removed(removed, &desk);
        self.live.replace(desk.clone());
        if let Err(e) = self.set_state(reconcile(self.state(), &desk)) {
            tracing::warn!("{e}");
        }
        self.desk_changed();
    }

    /// Lo que se borra con un monitor que ya no está en `remaining`: sus salidas quitadas
    /// vuelven y su perfil del usuario se borra, salvo que otro monitor del puesto sea del
    /// mismo modelo.
    pub(super) fn forget_removed(&self, removed: &[MonitorConfig], remaining: &Config) {
        if removed.is_empty() {
            return;
        }
        if let Some(desktop) = &self.desktop {
            crate::safety::reattach_removed(&self.ledger, desktop.0.as_ref(), removed);
        }
        let Some(dir) = self
            .user_profiles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            return;
        };
        let db = ProfileDb::load(Some(&dir));
        for m in removed {
            for id in m.models() {
                let Some(entry) = db.get(id) else { continue };
                let ProfileSource::User(path) = &entry.source else {
                    continue;
                };
                let shared = std::iter::once(&entry.profile.pnp_id)
                    .chain(&entry.profile.pnp_aliases)
                    .any(|p| remaining.monitors.iter().any(|r| r.is_model(p)));
                if !shared {
                    match std::fs::remove_file(path) {
                        Ok(()) => tracing::info!("perfil del usuario borrado: {}", path.display()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            tracing::warn!("no se pudo borrar {}: {e}", path.display());
                        }
                    }
                }
            }
        }
    }

    // ---- «Presiona una tecla» ----

    pub fn key_probe_status(&self) -> KeyProbeStatus {
        *self.key_probe.borrow()
    }

    pub fn subscribe_key_probe(&self) -> watch::Receiver<KeyProbeStatus> {
        self.key_probe.subscribe()
    }

    /// Las dos máquinas escuchan (o dejan de escuchar).
    pub(super) fn key_probe(&self, on: bool) {
        self.key_probe.send_replace(if on {
            KeyProbeStatus {
                armed: true,
                heard: None,
            }
        } else {
            KeyProbeStatus::default()
        });
        let msg = if on {
            KeyProbeMsg::Arm
        } else {
            KeyProbeMsg::Disarm
        };
        self.orch.peer().send(ControlMsg::KeyProbe(msg));
    }

    /// La ventana de aquí oyó una tecla: el teclado está en esta máquina.
    pub(super) fn heard_key(&self) {
        if self.hear(self.cfg.machine) {
            self.orch
                .peer()
                .send(ControlMsg::KeyProbe(KeyProbeMsg::Heard));
        }
    }

    /// Anota la primera tecla que llega. Falso si no se escucha o ya llegó una.
    fn hear(&self, machine: Machine) -> bool {
        self.key_probe.send_if_modified(|s| {
            if s.armed && s.heard.is_none() {
                s.heard = Some(machine);
                true
            } else {
                false
            }
        })
    }

    /// Lo que mandó la otra máquina para la prueba.
    pub fn receive_key_probe(&self, msg: KeyProbeMsg) {
        match msg {
            KeyProbeMsg::Arm => {
                self.key_probe.send_replace(KeyProbeStatus {
                    armed: true,
                    heard: None,
                });
            }
            KeyProbeMsg::Disarm => {
                self.key_probe.send_replace(KeyProbeStatus::default());
            }
            KeyProbeMsg::Heard => {
                self.hear(self.cfg.machine.other());
            }
        }
    }

    // ---- Dónde está el teclado ----

    /// Avisa cuando el usuario dice dónde está el teclado (o la configuración lo decía).
    pub fn subscribe_keyboard(&self) -> watch::Receiver<Option<Machine>> {
        self.keyboard.subscribe()
    }

    pub fn keyboard_declared(&self) -> Option<Machine> {
        *self.keyboard.borrow()
    }

    /// El usuario confirmó dónde está el teclado y el mouse: se guarda, se le dice a la
    /// otra máquina y el motor de entrada lo adopta.
    pub(super) fn set_keyboard(&self, machine: Machine) {
        self.declare_keyboard(machine);
        self.orch.peer().send(ControlMsg::Keyboard(machine));
    }

    /// Lo mismo, dicho por la otra máquina.
    pub fn receive_keyboard(&self, machine: Machine) {
        self.declare_keyboard(machine);
    }

    fn declare_keyboard(&self, machine: Machine) {
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_keyboard(&path, machine)
        {
            tracing::warn!(
                "no se guardó dónde está el teclado en {}: {e}",
                path.display()
            );
        }
        self.keyboard.send_replace(Some(machine));
        // Con motor de entrada, su tarea de estado escucha la declaración y publica el
        // estado; sin él (pruebas, backends simulados) se actualiza aquí.
        if !self.input_engine.load(std::sync::atomic::Ordering::Acquire) {
            let mut status = self.input_status();
            status.server = machine;
            status.defined = true;
            self.set_input_status(status);
        }
    }

    /// Hay un motor de entrada armando el estado: [`Agent::subscribe_keyboard`] lo despierta.
    pub fn set_input_engine(&self) {
        self.input_engine
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// La latencia del enlace con la otra máquina, para mostrarla en el asistente.
    pub(super) fn ping_peer(&self) -> Reply {
        match self.orch.peer().rtt() {
            Some((link, rtt)) => Reply::Pong {
                link,
                millis: u32::try_from(rtt.as_millis().max(1)).unwrap_or(u32::MAX),
            },
            None => Reply::Rejected("ningún enlace ve a la otra máquina".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tandem_core::{BuiltinDisplay, Input};
    use tandem_display::desktop::DesktopError;
    use tandem_proto::LinkBasis;

    use super::*;
    use crate::capabilities::Seen;
    use crate::config::AgentConfig;
    use crate::display::DisplaySteps;
    use crate::safety::DetachLedger;
    use crate::store::StateStore;

    /// Un escritorio que anota todo lo que se le pide.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl DesktopControl for Recorder {
        fn attach(&self, m: &MonitorConfig) -> Result<(), DesktopError> {
            self.0.lock().unwrap().push(format!("attach {}", m.id));
            Ok(())
        }
        fn detach(&self, m: &MonitorConfig) -> Result<(), DesktopError> {
            self.0.lock().unwrap().push(format!("detach {}", m.id));
            Ok(())
        }
        fn set_builtin(&self, _: BuiltinDisplay) -> Result<(), DesktopError> {
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct Fixed(Vec<Seen>);

    impl MonitorScanner for Fixed {
        fn scan(&self) -> Result<Vec<Seen>, String> {
            Ok(self.0.clone())
        }
    }

    fn pc_view() -> Vec<Seen> {
        vec![Seen {
            pnp_id: Some("SAM711A".into()),
            model: Some("LC34G55T".into()),
            name: "34\"".into(),
            serial: Some(7),
            inches: Some(34),
            output: Some(Input::DisplayPort),
        }]
    }

    /// Un agente de la PC recién instalado: puesto vacío, un escritorio que anota todo.
    fn fresh(dir: &std::path::Path) -> (Arc<Agent>, Arc<Recorder>) {
        let text = crate::autoconfig::generate(Machine::Pc);
        let file = dir.join("tandem.toml");
        std::fs::write(&file, &text).unwrap();
        let cfg = AgentConfig::from_toml(&text).unwrap();
        let desktop = Arc::new(Recorder::default());
        let steps = DisplaySteps::new(Machine::Pc, cfg.desk.clone()).with_desktop(desktop.clone());
        let agent = Agent::with_local_steps(
            cfg,
            StateStore::new(dir.join("state.json")),
            Arc::new(steps),
        );
        agent.set_config_file(file);
        agent.set_data_dir(dir);
        agent.set_scanner(Arc::new(Fixed(pc_view())));
        agent.set_seen_monitors(pc_view());
        (Arc::new(agent), desktop)
    }

    fn new_34() -> NewMonitor {
        NewMonitor {
            label: "34\"".into(),
            pnp_ids: vec!["SAM711A".into(), "SAM7119".into()],
            wiring: [
                (Machine::Pc, Input::DisplayPort),
                (Machine::Mac, Input::Hdmi),
            ]
            .into(),
        }
    }

    #[tokio::test]
    async fn a_clean_install_has_an_empty_desk_and_nothing_to_draw() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = fresh(dir.path());
        assert!(a.desk().monitors.is_empty());
        assert!(a.plan_desk().monitors.is_empty());
        assert!(a.state().owner.is_empty());
        assert!(!a.input_status().defined);
        // Detectar muestra lo conectado pero no agrega nada.
        let Reply::Scan(scan) = a.respond(tandem_proto::UiCommand::ScanMonitors).await else {
            panic!("sin escaneo")
        };
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(scan.candidates[0].basis, LinkBasis::Single);
        assert!(a.desk().monitors.is_empty());
    }

    #[tokio::test]
    async fn none_of_the_wizard_operations_ever_removes_an_output() {
        let dir = tempfile::tempdir().unwrap();
        let (a, desktop) = fresh(dir.path());
        a.respond(tandem_proto::UiCommand::ScanMonitors).await;
        assert_eq!(
            a.handle(tandem_proto::UiCommand::AddMonitors(vec![new_34()])),
            Reply::Done
        );
        assert_eq!(
            a.handle(tandem_proto::UiCommand::MarkManual {
                monitor: "sam711a".into()
            }),
            Reply::Done
        );
        a.handle(tandem_proto::UiCommand::KeyProbe { on: true });
        a.handle(tandem_proto::UiCommand::HeardKey);
        a.handle(tandem_proto::UiCommand::SetKeyboard(Machine::Pc));
        // El puesto que manda la otra máquina, con un monitor menos, tampoco.
        let mut theirs = a.desk_sync();
        theirs.rev += 1;
        theirs.desk.monitors.clear();
        a.receive_desk(theirs);
        assert!(a.desk().monitors.is_empty());
        assert_eq!(a.handle(tandem_proto::UiCommand::ResetDesk), Reply::Done);
        let calls = desktop.0.lock().unwrap().clone();
        assert!(calls.iter().all(|c| !c.starts_with("detach")), "{calls:?}");
    }

    #[tokio::test]
    async fn removing_a_monitor_brings_back_what_this_machine_had_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, desktop) = {
            let (a, d) = fresh(dir.path());
            (a, d)
        };
        assert_eq!(
            a.handle(tandem_proto::UiCommand::AddMonitors(vec![new_34()])),
            Reply::Done
        );
        // Esta máquina había quitado el 34" de su escritorio y anotó que lo hizo.
        let ledger = Arc::new(DetachLedger::in_memory());
        ledger.detached(&"sam711a".into());
        let inner = Arc::get_mut(&mut a).unwrap();
        inner.ledger = ledger.clone();
        inner.desktop = Some(DesktopHandle(desktop.clone()));
        assert_eq!(
            a.handle(tandem_proto::UiCommand::RemoveMonitor {
                monitor: "sam711a".into()
            }),
            Reply::Done
        );
        assert_eq!(*desktop.0.lock().unwrap(), ["attach sam711a"]);
        assert!(ledger.pending().is_empty());
    }

    #[tokio::test]
    async fn marking_a_monitor_manual_drops_its_ddc_codes_and_syncs() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = fresh(dir.path());
        a.handle(tandem_proto::UiCommand::AddMonitors(vec![new_34()]));
        assert!(a.desk().monitors[0].ddc.is_some());
        let rev = a.desk_sync().rev;
        a.handle(tandem_proto::UiCommand::MarkManual {
            monitor: "sam711a".into(),
        });
        assert!(a.desk().monitors[0].ddc.is_none());
        assert!(a.desk_sync().rev > rev);
        let saved = AgentConfig::load(&dir.path().join("tandem.toml")).unwrap();
        assert!(saved.desk.monitors[0].ddc.is_none());
        // Otra vez: nada que cambiar, no sube la versión.
        let rev = a.desk_sync().rev;
        a.handle(tandem_proto::UiCommand::MarkManual {
            monitor: "sam711a".into(),
        });
        assert_eq!(a.desk_sync().rev, rev);
        assert!(matches!(
            a.handle(tandem_proto::UiCommand::MarkManual {
                monitor: "otro".into()
            }),
            Reply::Rejected(_)
        ));
    }

    #[tokio::test]
    async fn adding_a_monitor_twice_or_with_bad_data_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = fresh(dir.path());
        assert_eq!(
            a.handle(tandem_proto::UiCommand::AddMonitors(vec![new_34()])),
            Reply::Done
        );
        let rev = a.desk_sync().rev;
        // El mismo monitor por su otra identidad.
        let again = NewMonitor {
            pnp_ids: vec!["SAM7119".into()],
            ..new_34()
        };
        assert!(matches!(
            a.handle(tandem_proto::UiCommand::AddMonitors(vec![again])),
            Reply::Rejected(_)
        ));
        assert_eq!(a.desk().monitors.len(), 1);
        assert_eq!(a.desk_sync().rev, rev);
        // Una lista vacía no cambia nada.
        assert_eq!(
            a.handle(tandem_proto::UiCommand::AddMonitors(vec![])),
            Reply::Done
        );
        assert_eq!(a.desk_sync().rev, rev);
    }

    #[tokio::test]
    async fn the_keyboard_is_undefined_until_someone_says_and_never_assumed_mac() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = fresh(dir.path());
        assert!(!a.input_status().defined);
        assert_eq!(a.keyboard_declared(), None);
        // Escuchar sin que llegue una tecla no define nada.
        a.handle(tandem_proto::UiCommand::KeyProbe { on: true });
        assert_eq!(a.key_probe_status().heard, None);
        // Una tecla sin escuchar se ignora.
        a.handle(tandem_proto::UiCommand::KeyProbe { on: false });
        a.handle(tandem_proto::UiCommand::HeardKey);
        assert_eq!(a.key_probe_status(), KeyProbeStatus::default());
        a.handle(tandem_proto::UiCommand::KeyProbe { on: true });
        a.handle(tandem_proto::UiCommand::HeardKey);
        assert_eq!(a.key_probe_status().heard, Some(Machine::Pc));
        a.handle(tandem_proto::UiCommand::SetKeyboard(Machine::Pc));
        assert!(a.input_status().defined);
        let saved = AgentConfig::load(&dir.path().join("tandem.toml")).unwrap();
        assert_eq!(saved.input.keyboard, Some(Machine::Pc));
    }
}
