//! Lo que el agente hace para el asistente de configuración inicial (DESIGN §5): la prueba
//! «presiona una tecla», dónde está el teclado y el ping del enlace.

use tandem_core::Machine;
use tandem_proto::{ControlMsg, KeyProbeMsg, KeyProbeStatus, Reply};
use tokio::sync::watch;

use super::Agent;

impl Agent {
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
        if self.hear(self.machine()) {
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
                self.hear(self.machine().other());
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
        let editing = self.edit();
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_keyboard(&path, machine)
        {
            tracing::warn!(
                "no se guardó dónde está el teclado en {}: {e}",
                path.display()
            );
        }
        self.keyboard.send_replace(Some(machine));
        drop(editing);
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
    use std::sync::Arc;

    use super::*;
    use crate::config::AgentConfig;
    use crate::store::StateStore;

    /// Un agente de la PC recién instalado, con su `tandem.toml` generado.
    fn fresh(dir: &std::path::Path) -> Arc<Agent> {
        let text = crate::autoconfig::generate(Machine::B);
        let file = dir.join("tandem.toml");
        std::fs::write(&file, &text).unwrap();
        let cfg = AgentConfig::from_toml(&text).unwrap();
        let agent = Agent::new(cfg, StateStore::new(dir.join("state.json")));
        agent.set_config_file(file);
        Arc::new(agent)
    }

    #[test]
    fn a_clean_install_has_an_empty_desk() {
        let dir = tempfile::tempdir().unwrap();
        let a = fresh(dir.path());
        assert_eq!(a.desk(), tandem_core::Config::default());
        assert!(!a.input_status().defined);
        assert_eq!(a.desk_sync().rev, 0);
    }

    #[tokio::test]
    async fn without_a_link_there_is_no_ping() {
        let dir = tempfile::tempdir().unwrap();
        let a = fresh(dir.path());
        assert!(matches!(
            a.handle(tandem_proto::UiCommand::PingPeer),
            Reply::Rejected(_)
        ));
    }

    #[tokio::test]
    async fn the_keyboard_is_undefined_until_someone_says_and_never_assumed_mac() {
        let dir = tempfile::tempdir().unwrap();
        let a = fresh(dir.path());
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
        assert_eq!(a.key_probe_status().heard, Some(Machine::B));
        a.handle(tandem_proto::UiCommand::SetKeyboard(Machine::B));
        assert!(a.input_status().defined);
        let saved = AgentConfig::load(&dir.path().join("tandem.toml")).unwrap();
        assert_eq!(saved.input.keyboard, Some(Machine::B));
    }
}
