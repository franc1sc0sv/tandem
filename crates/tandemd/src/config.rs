//! `tandem.toml`: el puesto ([`tandem_core::Config`]), la red y qué máquina es esta.
//!
//! ```toml
//! machine = "mac"
//!
//! [network]
//! port = 47770
//! peers = [
//!     { kind = "usb4", addr = "10.77.1.2:47770" },
//!     { kind = "ethernet", addr = "10.77.2.2:47770" },
//! ]
//!
//! [orchestrator]
//! power_on_timeout_secs = 120
//! step_timeout_secs = 10
//!
//! [power]
//! # Solo en la Mac: sin enlaces con la PC durante esto, se la da por apagada.
//! pc_lost_after_secs = 15
//!
//! # Solo en la Mac: Wake-on-LAN por el enlace Ethernet directo.
//! [power.wol]
//! mac = "9c:6b:00:1a:2b:ff"
//! source = "10.77.2.1"
//! broadcast = "10.77.2.3"
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
use tandem_link::discovery::{DiscoveryError, link_candidates, validate_static_links};
use tandem_link::manager::LinkCandidate;
use tandem_power::WolTarget;

/// Puerto UDP predeterminado del plano de control (QUIC).
pub const DEFAULT_PORT: u16 = 47770;

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
    /// El puesto: es el mismo archivo en las dos máquinas, salvo `machine`.
    #[serde(flatten)]
    pub desk: Config,
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
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            port: DEFAULT_PORT,
            peers: Vec::new(),
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

/// Encendido y apagado de la PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerConfig {
    /// Cuánto tiempo sin ningún enlace con la PC hace que la Mac la dé por apagada
    /// (un corte de corriente). Más corto que un reinicio de Windows no sirve de
    /// mucho, pero cualquier caída breve de los cables tiene que quedar por debajo.
    #[serde(default = "default_pc_lost_after")]
    pub pc_lost_after_secs: u64,
    /// Cómo despertar la PC. Sin esta sección, encenderla falla con un error claro.
    #[serde(default)]
    pub wol: Option<WolTarget>,
}

fn default_pc_lost_after() -> u64 {
    15
}

impl Default for PowerConfig {
    fn default() -> Self {
        PowerConfig {
            pc_lost_after_secs: default_pc_lost_after(),
            wol: None,
        }
    }
}

impl PowerConfig {
    pub fn pc_lost_after(&self) -> Duration {
        Duration::from_secs(self.pc_lost_after_secs)
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
    #[error("los tiempos máximos de los pasos no pueden ser 0")]
    ZeroTimeout,
    #[error(
        "power.pc_lost_after_secs no puede ser 0: cualquier parpadeo de los enlaces daría la PC por apagada"
    )]
    ZeroPcLostAfter,
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
        validate_static_links(&self.network.peers)?;
        let o = &self.orchestrator;
        if o.power_on_timeout_secs == 0 || o.step_timeout_secs == 0 {
            return Err(ConfigError::ZeroTimeout);
        }
        if self.power.pc_lost_after_secs == 0 {
            return Err(ConfigError::ZeroPcLostAfter);
        }
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
            fixtures::desk().power.pc_lost_after(),
            Duration::from_secs(15)
        );
        let base = "machine = \"mac\"\nmonitors = []\n[power]\n";
        let cfg = AgentConfig::from_toml(&format!("{base}pc_lost_after_secs = 40")).unwrap();
        assert_eq!(cfg.power.pc_lost_after(), Duration::from_secs(40));
        let err = AgentConfig::from_toml(&format!("{base}pc_lost_after_secs = 0")).unwrap_err();
        assert!(matches!(err, ConfigError::ZeroPcLostAfter), "{err}");
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
    fn load_reports_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = AgentConfig::load(&dir.path().join("no-existe.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }));
    }
}
