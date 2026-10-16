//! `tandem.toml`: el puesto ([`tandem_core::Config`]), la red y qué máquina es esta.
//!
//! ```toml
//! machine = "mac"
//!
//! [network]
//! port = 52000
//! peers = [
//!     { kind = "usb4", addr = "10.77.1.2:52000" },
//!     { kind = "ethernet", addr = "10.77.2.2:52000" },
//! ]
//!
//! [orchestrator]
//! power_on_timeout_secs = 120
//! step_timeout_secs = 10
//!
//! [power]
//! # Sin enlaces con la otra máquina durante esto, se la da por apagada (las dos vigilan).
//! peer_lost_after_secs = 15
//! # Esta máquina se despierta por Wake-on-LAN (lo anuncia a la otra).
//! wakeable = true
//!
//! # Cómo despertar a la otra máquina: Wake-on-LAN por el enlace Ethernet directo.
//! [power.wol]
//! mac = "9c:6b:00:1a:2b:ff"
//! source = "10.77.2.1"
//! broadcast = "10.77.2.3"
//!
//! [input]
//! # Dónde está el teclado y mouse al arrancar, hasta que el hub decida.
//! keyboard = "mac"
//! # Bloquea el cursor en la máquina donde está (nombres de `KeyboardEvent.code`).
//! lock_shortcut = ["ScrollLock"]
//! clipboard = true
//!
//! # El teclado y el mouse del hub, por VID:PID. Sin esta sección no hay roles automáticos.
//! [input.hub]
//! keyboard = "046d:c33f"
//! mouse = "046d:c539"
//!
//! [[monitors]]
//! id = "m34"
//! label = '34"'
//! wiring = { pc = "displayport", mac = "hdmi" }
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tandem_core::{Config, Machine};
use tandem_input::HidUsage;
use tandem_input::event::RemapPolicy;
use tandem_input::event::keymap::from_code_name;
use tandem_link::discovery::{DiscoveryError, link_candidates, validate_static_links};
use tandem_link::manager::LinkCandidate;
use tandem_peripherals::HubConfig;
use tandem_power::WolTarget;

/// Puerto UDP predeterminado del plano de control (QUIC). Fuera de 47000–48999: Windows
/// reserva ese rango en las PCs con WSL o Hyper-V (el 47770 de antes daba `os error 10048`).
pub const DEFAULT_PORT: u16 = 52000;

/// Los que se prueban, en orden, si el configurado no se puede abrir
/// ([`crate::network::bind_first`]). Repartidos en varios bloques, porque Windows reserva
/// los puertos de a rangos, y todos fuera de 47000–48999. Cada uno deja libre el siguiente
/// para el emparejamiento.
pub const FALLBACK_PORTS: [u16; 8] = [52000, 52100, 53100, 54200, 55300, 56400, 45500, 44600];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Máquina donde corre este agente.
    pub machine: Machine,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
    #[serde(default)]
    pub power: PowerConfig,
    #[serde(default)]
    pub input: InputConfig,
    /// Versión de la configuración compartida del puesto (`[[monitors]]` y el atajo de
    /// bloqueo), que se sincroniza con la otra máquina ([`crate::desk_sync`]). La
    /// escribe el agente; 0 = nunca se cambió desde que se generó.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub desk_rev: u64,
    /// Qué máquina hizo el último cambio de `desk_rev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desk_origin: Option<Machine>,
    /// El puesto: es el mismo en las dos máquinas, salvo `machine`.
    #[serde(flatten)]
    pub desk: Config,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    /// Direcciones fijas del otro agente, una por enlace (los directos:
    /// USB4 y Ethernet). La de la LAN se descubre por mDNS si no se fija.
    #[serde(default)]
    pub peers: Vec<LinkCandidate>,
    /// Puerto UDP donde la PC espera mientras se empareja (y al que la Mac le pide
    /// emparejarse por los enlaces fijos). Por omisión, el siguiente a `port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_port: Option<u16>,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            port: DEFAULT_PORT,
            peers: Vec::new(),
            pairing_port: None,
        }
    }
}

/// Tiempos máximos de los pasos de un cambio de modo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorConfig {
    /// Encender la PC y esperar a que su agente responda.
    #[serde(default = "default_power_on_timeout")]
    pub power_on_timeout_secs: u64,
    /// Cualquier otro paso.
    #[serde(default = "default_step_timeout")]
    pub step_timeout_secs: u64,
}

fn default_power_on_timeout() -> u64 {
    120
}

fn default_step_timeout() -> u64 {
    10
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        OrchestratorConfig {
            power_on_timeout_secs: default_power_on_timeout(),
            step_timeout_secs: default_step_timeout(),
        }
    }
}

impl OrchestratorConfig {
    pub fn timeouts(&self) -> crate::orchestrator::StepTimeouts {
        crate::orchestrator::StepTimeouts {
            power_on: Duration::from_secs(self.power_on_timeout_secs),
            step: Duration::from_secs(self.step_timeout_secs),
        }
    }
}

/// Encendido y apagado de las dos máquinas (DESIGN §3.7: cualquiera puede ser la principal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerConfig {
    /// Cuánto tiempo sin ningún enlace con la otra máquina hace que esta la dé por
    /// apagada (un corte de corriente, o se suspendió). Más corto que un reinicio no
    /// sirve de mucho, pero cualquier caída breve de los cables tiene que quedar por
    /// debajo. `pc_lost_after_secs` es el nombre de antes.
    #[serde(default = "default_peer_lost_after", alias = "pc_lost_after_secs")]
    pub peer_lost_after_secs: u64,
    /// Esta máquina se despierta por Wake-on-LAN; se le anuncia a la otra.
    #[serde(default = "default_true")]
    pub wakeable: bool,
    /// Cómo despertar a la otra máquina. Sin esta sección, encenderla falla con un error
    /// claro.
    #[serde(default)]
    pub wol: Option<WolTarget>,
}

fn default_peer_lost_after() -> u64 {
    15
}

impl Default for PowerConfig {
    fn default() -> Self {
        PowerConfig {
            peer_lost_after_secs: default_peer_lost_after(),
            wakeable: true,
            wol: None,
        }
    }
}

impl PowerConfig {
    pub fn peer_lost_after(&self) -> Duration {
        Duration::from_secs(self.peer_lost_after_secs)
    }
}

/// Teclado, mouse y portapapeles compartidos (DESIGN §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Dónde está el teclado y mouse al arrancar, hasta que el hub (o el otro agente) diga
    /// otra cosa. `server` es el nombre de antes.
    #[serde(default = "default_keyboard", alias = "server")]
    pub keyboard: Machine,
    /// El teclado y el mouse del hub. Sin esto los roles no cambian solos.
    #[serde(default)]
    pub hub: Option<HubConfig>,
    /// Reasignación de modificadores por dirección; por omisión Cmd↔Ctrl.
    #[serde(default)]
    pub remap: RemapPolicy,
    /// Teclas (nombres de `KeyboardEvent.code`, todas a la vez) que bloquean el cursor
    /// en la máquina donde está. Vacío = sin atajo.
    #[serde(default = "default_lock_shortcut")]
    pub lock_shortcut: Vec<String>,
    /// Sincronizar el portapapeles.
    #[serde(default = "default_true")]
    pub clipboard: bool,
}

fn default_keyboard() -> Machine {
    Machine::Mac
}

/// ScrollLock, como en Synergy/Deskflow. En la Mac llega como F14 y vale igual.
fn default_lock_shortcut() -> Vec<String> {
    vec!["ScrollLock".to_owned()]
}

fn default_true() -> bool {
    true
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            keyboard: default_keyboard(),
            hub: None,
            remap: RemapPolicy::default(),
            lock_shortcut: default_lock_shortcut(),
            clipboard: true,
        }
    }
}

impl InputConfig {
    /// Las teclas del atajo de bloqueo. La config ya validada no falla.
    pub fn lock_keys(&self) -> Result<Vec<HidUsage>, ConfigError> {
        self.lock_shortcut
            .iter()
            .map(|name| from_code_name(name).ok_or_else(|| ConfigError::UnknownKey(name.clone())))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no se pudo leer {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("configuración inválida: {0}")]
    Parse(#[from] toml::de::Error),
    #[error(transparent)]
    Desk(#[from] tandem_core::model::ConfigError),
    #[error("el puerto de red no puede ser 0")]
    ZeroPort,
    #[error("el puerto de emparejamiento ({0}) no puede ser 0 ni el mismo que el de red")]
    BadPairingPort(u16),
    #[error("los tiempos máximos de los pasos no pueden ser 0")]
    ZeroTimeout,
    #[error(
        "power.peer_lost_after_secs no puede ser 0: cualquier parpadeo de los enlaces daría a la otra máquina por apagada"
    )]
    ZeroPeerLostAfter,
    #[error("el puerto del Wake-on-LAN no puede ser 0")]
    ZeroWolPort,
    #[error(
        "Wake-on-LAN: el origen {source_ip} y el destino {broadcast} no son de la misma familia (IPv4/IPv6)"
    )]
    WolFamilyMismatch {
        source_ip: std::net::IpAddr,
        broadcast: std::net::IpAddr,
    },
    #[error(transparent)]
    Peers(#[from] DiscoveryError),
    #[error(
        "input.lock_shortcut: «{0}» no es una tecla (se usan los nombres de KeyboardEvent.code, como ScrollLock o KeyL)"
    )]
    UnknownKey(String),
}

impl AgentConfig {
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let cfg: AgentConfig = toml::from_str(s)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::from_toml(&text)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.desk.validate()?;
        if self.network.port == 0 {
            return Err(ConfigError::ZeroPort);
        }
        let pairing = self.network.pairing_port();
        if pairing == 0 || pairing == self.network.port {
            return Err(ConfigError::BadPairingPort(pairing));
        }
        validate_static_links(&self.network.peers)?;
        let o = &self.orchestrator;
        if o.power_on_timeout_secs == 0 || o.step_timeout_secs == 0 {
            return Err(ConfigError::ZeroTimeout);
        }
        if self.power.peer_lost_after_secs == 0 {
            return Err(ConfigError::ZeroPeerLostAfter);
        }
        self.input.lock_keys()?;
        if let Some(wol) = &self.power.wol {
            if wol.port == 0 {
                return Err(ConfigError::ZeroWolPort);
            }
            if let Some(source_ip) = wol.source
                && source_ip.is_ipv4() != wol.broadcast.is_ipv4()
            {
                return Err(ConfigError::WolFamilyMismatch {
                    source_ip,
                    broadcast: wol.broadcast,
                });
            }
        }
        Ok(())
    }
}

impl NetworkConfig {
    /// Candidatos para el gestor de enlaces: los fijos de la config más lo
    /// descubierto en la LAN (`discovered`, de la mejor a la peor dirección).
    pub fn link_candidates(&self, discovered: &[std::net::SocketAddr]) -> Vec<LinkCandidate> {
        link_candidates(&self.peers, discovered)
    }

    /// Ver [`NetworkConfig::pairing_port`](field@NetworkConfig::pairing_port).
    pub fn pairing_port(&self) -> u16 {
        self.pairing_port
            .unwrap_or_else(|| self.port.checked_add(1).unwrap_or(self.port - 1))
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub const DESK: &str = r#"
        machine = "mac"

        [network]
        port = 47771
        peers = [
            { kind = "usb4", addr = "10.77.1.2:47770" },
            { kind = "ethernet", addr = "[fe80::1]:47770" },
        ]

        [orchestrator]
        step_timeout_secs = 10

        [[monitors]]
        id = "m34"
        label = '34"'
        edid_model = "LC34G55T"
        wiring = { pc = "displayport", mac = "hdmi" }
        ddc = { hdmi = 0x06, display_port = 0x0F }

        [[monitors]]
        id = "m32"
        label = '32"'
        wiring = { pc = "displayport", mac = "hdmi" }
    "#;

    pub fn desk() -> AgentConfig {
        AgentConfig::from_toml(DESK).unwrap()
    }

    /// El puesto con `[power.wol]`: esta máquina puede despertar a la otra.
    pub fn desk_with_wol() -> AgentConfig {
        let mut cfg = desk();
        cfg.power.wol = Some(tandem_power::WolTarget::new(
            "9c:6b:00:1a:2b:ff".parse().unwrap(),
        ));
        cfg
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::Input;
    use tandem_link::manager::LinkKind;

    use super::*;

    #[test]
    fn parses_full_config() {
        let cfg = fixtures::desk();
        assert_eq!(cfg.machine, Machine::Mac);
        assert_eq!(cfg.network.port, 47771);
        assert_eq!(cfg.network.peers.len(), 2);
        assert_eq!(cfg.desk.monitors.len(), 2);
        let m34 = cfg.desk.monitor(&"m34".into()).unwrap();
        assert_eq!(m34.input_for(Machine::Pc), Some(Input::DisplayPort));
        assert_eq!(m34.ddc.as_ref().unwrap().display_port, 0x0F);
    }

    #[test]
    fn network_is_optional() {
        let cfg = AgentConfig::from_toml(
            r#"
            machine = "pc"
            [[monitors]]
            id = "a"
            label = "a"
            wiring = { pc = "hdmi" }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.network, NetworkConfig::default());
        assert_eq!(cfg.network.port, DEFAULT_PORT);
        assert_eq!(cfg.orchestrator, OrchestratorConfig::default());
        let t = cfg.orchestrator.timeouts();
        assert_eq!(t.power_on, Duration::from_secs(120));
        assert_eq!(t.step, Duration::from_secs(10));
    }

    #[test]
    fn timeouts_are_configurable() {
        let cfg = AgentConfig::from_toml(
            "machine = \"mac\"\nmonitors = []\n[orchestrator]\npower_on_timeout_secs = 90\nstep_timeout_secs = 5",
        )
        .unwrap();
        let t = cfg.orchestrator.timeouts();
        assert_eq!(t.power_on, Duration::from_secs(90));
        assert_eq!(t.step, Duration::from_secs(5));
    }

    #[test]
    fn rejects_zero_timeout() {
        for field in ["power_on_timeout_secs", "step_timeout_secs"] {
            let err = AgentConfig::from_toml(&format!(
                "machine = \"mac\"\nmonitors = []\n[orchestrator]\n{field} = 0"
            ))
            .unwrap_err();
            assert!(matches!(err, ConfigError::ZeroTimeout), "{err}");
        }
    }

    #[test]
    fn rejects_unknown_orchestrator_field() {
        let err =
            AgentConfig::from_toml("machine = \"mac\"\nmonitors = []\n[orchestrator]\ntimeout = 1")
                .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn rejects_missing_machine() {
        let err = AgentConfig::from_toml("monitors = []").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn rejects_unknown_machine() {
        let err = AgentConfig::from_toml("machine = \"linux\"\nmonitors = []").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn rejects_duplicate_monitor() {
        let err = AgentConfig::from_toml(
            r#"
            machine = "mac"
            [[monitors]]
            id = "a"
            label = "a"
            wiring = { pc = "hdmi" }
            [[monitors]]
            id = "a"
            label = "b"
            wiring = { mac = "hdmi" }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Desk(tandem_core::model::ConfigError::DuplicateMonitor(_))
            ),
            "{err}"
        );
    }

    #[test]
    fn rejects_port_zero() {
        let err = AgentConfig::from_toml("machine = \"mac\"\nmonitors = []\n[network]\nport = 0")
            .unwrap_err();
        assert!(matches!(err, ConfigError::ZeroPort));
    }

    #[test]
    fn pairing_port_defaults_to_the_next_one() {
        let cfg = |net: &str| {
            AgentConfig::from_toml(&format!(
                "machine = \"pc\"\nmonitors = []\n[network]\n{net}"
            ))
        };
        assert_eq!(cfg("port = 47770").unwrap().network.pairing_port(), 47771);
        assert_eq!(cfg("port = 65535").unwrap().network.pairing_port(), 65534);
        let fixed = cfg("port = 47770\npairing_port = 50000").unwrap();
        assert_eq!(fixed.network.pairing_port(), 50000);
        for bad in ["pairing_port = 0", "port = 5000\npairing_port = 5000"] {
            assert!(
                matches!(cfg(bad), Err(ConfigError::BadPairingPort(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_peer_without_port() {
        let err = AgentConfig::from_toml(
            "machine = \"mac\"\nmonitors = []\n[network]\npeers = [{ kind = \"usb4\", addr = \"10.0.0.1:0\" }]",
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Peers(DiscoveryError::StaticWithoutPort(_))
            ),
            "{err}"
        );
    }

    #[test]
    fn rejects_peer_without_kind() {
        let err = AgentConfig::from_toml(
            "machine = \"mac\"\nmonitors = []\n[network]\npeers = [\"10.0.0.1:47770\"]",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn rejects_duplicate_peer_link() {
        let err = AgentConfig::from_toml(
            r#"
            machine = "mac"
            monitors = []
            [network]
            peers = [
                { kind = "usb4", addr = "10.77.1.2:47770" },
                { kind = "usb4", addr = "10.77.1.6:47770" },
            ]
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Peers(DiscoveryError::DuplicateStatic(LinkKind::Usb4))
            ),
            "{err}"
        );
    }

    #[test]
    fn static_peers_become_link_candidates() {
        let cfg = fixtures::desk();
        let lan = "192.168.1.20:47770".parse().unwrap();
        let got = cfg.network.link_candidates(&[lan]);
        let kinds: Vec<LinkKind> = got.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [LinkKind::Usb4, LinkKind::Ethernet, LinkKind::Wifi]);
        assert_eq!(got[0].addr, "10.77.1.2:47770".parse().unwrap());
        assert_eq!(got[1].addr, "[fe80::1]:47770".parse().unwrap());
        assert_eq!(got[2].addr, lan);
    }

    #[test]
    fn rejects_unknown_network_field() {
        let err = AgentConfig::from_toml("machine = \"mac\"\nmonitors = []\n[network]\nprot = 1")
            .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn power_is_optional() {
        assert_eq!(fixtures::desk().power, PowerConfig::default());
        assert_eq!(fixtures::desk().power.wol, None);
    }

    #[test]
    fn pc_lost_threshold_is_configurable_but_not_zero() {
        assert_eq!(
            fixtures::desk().power.peer_lost_after(),
            Duration::from_secs(15)
        );
        let base = "machine = \"mac\"\nmonitors = []\n[power]\n";
        let cfg = AgentConfig::from_toml(&format!("{base}peer_lost_after_secs = 40")).unwrap();
        assert_eq!(cfg.power.peer_lost_after(), Duration::from_secs(40));
        let err = AgentConfig::from_toml(&format!("{base}peer_lost_after_secs = 0")).unwrap_err();
        assert!(matches!(err, ConfigError::ZeroPeerLostAfter), "{err}");
    }

    #[test]
    fn parses_wake_on_lan() {
        let cfg = AgentConfig::from_toml(
            "machine = \"mac\"\nmonitors = []\n[power.wol]\nmac = \"9c-6b-00-1a-2b-ff\"\nsource = \"10.77.2.1\"\nbroadcast = \"10.77.2.3\"",
        )
        .unwrap();
        let wol = cfg.power.wol.unwrap();
        assert_eq!(wol.mac.to_string(), "9c:6b:00:1a:2b:ff");
        assert_eq!(wol.destination(), "10.77.2.3:9".parse().unwrap());
    }

    #[test]
    fn rejects_bad_wake_on_lan() {
        let base = "machine = \"mac\"\nmonitors = []\n[power.wol]\nmac = \"9c:6b:00:1a:2b:ff\"\n";
        let err = AgentConfig::from_toml(&format!("{base}port = 0")).unwrap_err();
        assert!(matches!(err, ConfigError::ZeroWolPort), "{err}");
        let err = AgentConfig::from_toml(&format!("{base}source = \"fe80::1\"")).unwrap_err();
        assert!(
            matches!(err, ConfigError::WolFamilyMismatch { .. }),
            "{err}"
        );
        let err = AgentConfig::from_toml(
            "machine = \"mac\"\nmonitors = []\n[power.wol]\nmac = \"no-es-una-mac\"",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
        let err = AgentConfig::from_toml("machine = \"mac\"\nmonitors = []\n[power]\nwake = 1")
            .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn input_defaults() {
        let cfg = fixtures::desk();
        assert_eq!(cfg.input, InputConfig::default());
        assert_eq!(cfg.input.keyboard, Machine::Mac);
        assert!(cfg.input.clipboard && cfg.input.hub.is_none());
        assert_eq!(cfg.input.lock_keys().unwrap(), [HidUsage::key(0x47)]);
        assert_eq!(cfg.input.remap, RemapPolicy::default());
    }

    #[test]
    fn parses_input() {
        let cfg = AgentConfig::from_toml(
            r#"
            machine = "pc"
            monitors = []
            [input]
            keyboard = "pc"
            lock_shortcut = ["ControlLeft", "KeyL"]
            clipboard = false
            [input.hub]
            keyboard = "046d:c33f"
            mouse = "046d:c539"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.input.keyboard, Machine::Pc);
        // El nombre de antes sigue valiendo.
        let old = AgentConfig::from_toml(
            "machine = \"mac\"\nmonitors = []\n[input]\nserver = \"pc\"\n[power]\npc_lost_after_secs = 9",
        )
        .unwrap();
        assert_eq!(old.input.keyboard, Machine::Pc);
        assert_eq!(old.power.peer_lost_after_secs, 9);
        assert!(!cfg.input.clipboard);
        assert_eq!(
            cfg.input.lock_keys().unwrap(),
            [HidUsage::key(0xE0), HidUsage::key(0x0F)]
        );
        let hub = cfg.input.hub.unwrap();
        assert_eq!(hub.keyboard.to_string(), "046d:c33f");
        let none =
            AgentConfig::from_toml("machine = \"mac\"\nmonitors = []\n[input]\nlock_shortcut = []")
                .unwrap();
        assert!(none.input.lock_keys().unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_input() {
        let base = "machine = \"mac\"\nmonitors = []\n[input]\n";
        let err = AgentConfig::from_toml(&format!("{base}lock_shortcut = [\"Bloq\"]")).unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownKey(ref k) if k == "Bloq"),
            "{err}"
        );
        let err = AgentConfig::from_toml(&format!("{base}sever = \"pc\"")).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
        let err =
            AgentConfig::from_toml(&format!("{base}[input.hub]\nkeyboard = \"1:2\"")).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn load_reports_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = AgentConfig::load(&dir.path().join("no-existe.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }));
    }
}
