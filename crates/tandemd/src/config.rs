//! `tandem.toml`: el puesto compartido ([`tandem_core::Config`]), la red y qué lado es
//! esta máquina.
//!
//! ```toml
//! # El lado de esta máquina: lo decide la clave pública al emparejar (la menor es `a`).
//! machine = "a"
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
//! mac = "aa:bb:cc:dd:ee:ff"
//! source = "10.77.2.1"
//! broadcast = "10.77.2.3"
//!
//! [input]
//! # Dónde está el teclado y mouse al arrancar, hasta que el hub decida.
//! keyboard = "a"
//!
//! # El teclado y el mouse del hub, por VID:PID. Sin esta sección no hay roles automáticos.
//! [input.hub]
//! keyboard = "046d:c33f"
//! mouse = "046d:c539"
//!
//! # Qué máquina es cada lado: lo llena el agente con lo que anuncia cada una.
//! [machines.a]
//! os = "macos"
//! name = "MacBook Pro"
//! color = "#7aa7e6"
//!
//! # El acomodo de pantallas (DESIGN §7). Sin `[[screens]]` de una máquina valen las que
//! # ella misma lee; sin `[arrangement]`, A a la izquierda de B.
//! [[screens]]
//! machine = "b"
//! id = "34"
//! x = 0
//! y = 0
//! width = 3440
//! height = 1440
//! scale = 1.0
//!
//! # De qué lado de B está A (left, right, up, down) y la alineación (start, center, end).
//! [arrangement]
//! a_side = "left"
//! align = "center"
//!
//! # Bloquea el cursor en la máquina donde está (nombres de `KeyboardEvent.code`).
//! [shortcuts]
//! lock = ["ScrollLock"]
//!
//! [prefs]
//! cmd_ctrl = true
//! drag_files = true
//!
//! # Módulos que el usuario apagó en cada máquina (los demás, encendidos).
//! [modules.b]
//! clipboard = false
//! ```
//!
//! Todo lo que no es de esta máquina (`[machines]`, `[[screens]]`, `[arrangement]`,
//! `[[scenes]]`, `[shortcuts]`, `[prefs]`, `[modules]`) es el puesto compartido: se edita
//! desde la UI (D4) y lo escribe el agente.
//!
//! Una configuración de v0.1 con `[[monitors]]` sigue cargando: la sección se ignora con
//! un aviso en el log ([`AgentConfig::warnings`]). Una de v0.2, con los lados `mac`/`pc`,
//! se traduce sola ([`crate::migrate`]), y una de antes de T-099 (atajo y portapapeles en
//! `[input]`, desplazamiento en `[arrangement]`) también.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tandem_core::{Config, Machine, Os};
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
    /// El lado de esta máquina. Lo decide la identidad ([`AgentConfig::with_side`]); lo
    /// guardado es lo último que se resolvió, y sin par todavía es `A`.
    #[serde(default = "default_side")]
    pub machine: Machine,
    /// El sistema de esta máquina. No se guarda: sale del binario ([`Os::local`]); las
    /// pruebas y la simulación lo cambian para armar puestos de cualquier combinación.
    #[serde(skip, default = "Os::local")]
    pub os: Os,
    /// Los lados del archivo no son los que valen: tenía los de v0.2 (`mac`/`pc`,
    /// [`crate::migrate`]) o la identidad dijo que esta máquina es el otro lado. El agente lo
    /// reescribe al conocer el archivo.
    #[serde(skip)]
    pub rewrite_sides: bool,
    /// El puesto venía con el formato de antes de T-099 ([`crate::migrate::desk_v03`]): su
    /// huella ya no dice nada y el agente lo reescribe al conocer el archivo, sin contarlo
    /// como un cambio.
    #[serde(skip)]
    pub legacy_desk: bool,
    /// La migración pasó al puesto algo que era de esta máquina (el portapapeles apagado) o
    /// le puso las escenas por omisión (T-100): cuenta como un cambio suyo, con versión
    /// nueva, para que la otra lo adopte.
    #[serde(skip)]
    pub local_to_desk: bool,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
    #[serde(default)]
    pub power: PowerConfig,
    #[serde(default)]
    pub input: InputConfig,
    /// Versión del puesto compartido, que se sincroniza con la otra máquina
    /// ([`tandem_core::DeskVersion`]). La escribe el agente; 0 = nunca se cambió desde que se
    /// generó.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub desk_rev: u64,
    /// Cuándo se hizo (milisegundos desde la época Unix, reloj de quien lo hizo).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub desk_at: u64,
    /// Qué máquina hizo el último cambio de `desk_rev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desk_origin: Option<Machine>,
    /// La huella del puesto que escribió el agente con `desk_rev`
    /// ([`crate::desk_sync::fingerprint`]). Si no coincide, alguien lo editó a mano: es un
    /// cambio de esta máquina y se le manda a la otra.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desk_sum: Option<String>,
    /// Cuántos `[[monitors]]` de v0.1 trae: se lee solo para avisar que ya no se usa.
    #[serde(
        default,
        rename = "monitors",
        deserialize_with = "count_entries",
        skip_serializing
    )]
    pub legacy_monitors: Option<usize>,
    /// El puesto: es el mismo en las dos máquinas, salvo `machine`.
    #[serde(flatten)]
    pub desk: Config,
}

fn default_side() -> Machine {
    Machine::A
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn count_entries<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    Ok(Some(match toml::Value::deserialize(d)? {
        toml::Value::Array(a) => a.len(),
        _ => 1,
    }))
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

/// Tiempos máximos de los pasos de una transacción (energía, monitores, escenas).
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
            monitor: crate::orchestrator::MONITOR_STEP,
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

/// Teclado y mouse de esta máquina (DESIGN §7). Lo compartido está en el puesto.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Dónde está el teclado y mouse al arrancar, hasta que el hub (o el otro agente) diga
    /// otra cosa. `server` es el nombre de antes.
    /// Sin él (la configuración que genera T-087 no lo escribe) no se sabe: el motor arranca
    /// con el lado A, pero la UI dice «sin definir» hasta que el hub o el asistente lo
    /// digan ([`InputConfig::keyboard`], [`InputConfig::keyboard_defined`]).
    #[serde(default, alias = "server", skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<Machine>,
    /// El teclado y el mouse del hub. Sin esto los roles no cambian solos.
    #[serde(default)]
    pub hub: Option<HubConfig>,
    /// La tabla de cada dirección de Cmd↔Ctrl; si se reasigna o no lo dice el puesto
    /// (`[prefs] cmd_ctrl`).
    #[serde(default)]
    pub remap: RemapPolicy,
    /// Si un arrastre (un botón del mouse presionado) cruza a la otra máquina. Por
    /// omisión no, como Deskflow (T-093).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drag_between_machines: bool,
}

/// Las dos máquinas tienen que arrancar con el mismo servidor aunque ninguna sepa nada: el
/// lado A, que las dos conocen sin hablarse.
fn default_keyboard() -> Machine {
    Machine::A
}

impl InputConfig {
    /// Dónde arranca el motor de entrada: lo configurado o, sin definir, el lado A. Para la
    /// UI, mira [`InputConfig::keyboard_defined`].
    pub fn keyboard(&self) -> Machine {
        self.keyboard.unwrap_or_else(default_keyboard)
    }

    /// Si el teclado está dicho en la configuración.
    pub fn keyboard_defined(&self) -> bool {
        self.keyboard.is_some()
    }
}

fn default_true() -> bool {
    true
}

/// Las teclas de un atajo del puesto (`[shortcuts]`), por su nombre de `KeyboardEvent.code`.
pub fn shortcut_keys(names: &[String]) -> Result<Vec<HidUsage>, ConfigError> {
    names
        .iter()
        .map(|name| from_code_name(name).ok_or_else(|| ConfigError::UnknownKey(name.clone())))
        .collect()
}

/// Todas las teclas de los atajos del puesto (el de bloqueo y los de las escenas) existen.
pub fn desk_keys(desk: &tandem_core::Desk) -> Result<(), ConfigError> {
    shortcut_keys(&desk.shortcuts.lock)?;
    for scene in &desk.scenes {
        shortcut_keys(&scene.shortcut)?;
    }
    Ok(())
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
        "shortcuts.lock: «{0}» no es una tecla (se usan los nombres de KeyboardEvent.code, como ScrollLock o KeyL)"
    )]
    UnknownKey(String),
}

impl AgentConfig {
    /// Lee y valida. Un archivo de v0.2 (lados `mac`/`pc`) se traduce primero, con esta
    /// máquina como `A` hasta que la identidad diga otra cosa ([`crate::migrate`]).
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let mut table: toml::Table = s.parse()?;
        // Primero el puesto: decide si el acomodo era el de omisión con los nombres que
        // trae el archivo, antes de traducirlos.
        let migrated = crate::migrate::desk_v03(&mut table);
        let legacy = crate::migrate::sides_v02(&mut table);
        // Un puesto de antes de T-100 no tiene la clave `scenes` (uno sin escenas a propósito
        // tiene `scenes = []`).
        let before_scenes = !table.contains_key("scenes");
        let mut cfg: AgentConfig = table.try_into()?;
        cfg.rewrite_sides = legacy;
        cfg.legacy_desk = migrated.changed;
        cfg.local_to_desk = migrated.local;
        // Ya emparejado: las escenas por omisión reemplazan a los modos (D6). Es un cambio de
        // esta máquina, con versión nueva: se guarda y llega a la otra.
        if before_scenes && tandem_core::scene::add_defaults(&mut cfg.desk) {
            cfg.local_to_desk = true;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    /// La misma configuración vista desde el lado `side`: si no es el guardado, se cambian
    /// los lados de todo lo que los nombra (el puesto, el teclado, el autor del último
    /// cambio). Nada físico cambia: cada pantalla sigue siendo de su máquina.
    pub fn with_side(mut self, side: Machine) -> Self {
        if side == self.machine {
            return self;
        }
        self.machine = side;
        self.desk = self.desk.swap_sides();
        self.input.keyboard = self.input.keyboard.map(Machine::other);
        self.desk_origin = self.desk_origin.map(Machine::other);
        self.rewrite_sides = true;
        self
    }

    /// Lee `path` y deja en el log lo que se ignora ([`AgentConfig::warnings`]).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let cfg = Self::from_toml(&text)?;
        for w in cfg.warnings() {
            tracing::warn!("{}: {w}", path.display());
        }
        Ok(cfg)
    }

    /// Lo que la configuración trae y ya no se usa. Un `monitors = []` vacío (lo que
    /// generaba v0.1) no molesta a nadie y no se avisa.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.legacy_monitors.is_some_and(|n| n > 0) {
            out.push(
                "la sección [[monitors]] ya no se usa (v0.2 quitó el control de monitores): se ignora"
                    .to_owned(),
            );
        }
        out
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
        desk_keys(&self.desk)?;
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
        machine = "a"

        [network]
        port = 47771
        peers = [
            { kind = "usb4", addr = "10.77.1.2:47770" },
            { kind = "ethernet", addr = "[fe80::1]:47770" },
        ]

        [orchestrator]
        step_timeout_secs = 10
    "#;

    pub fn desk() -> AgentConfig {
        AgentConfig::from_toml(DESK).unwrap()
    }

    /// El puesto con `[power.wol]`: esta máquina puede despertar a la otra.
    pub fn desk_with_wol() -> AgentConfig {
        let mut cfg = desk();
        cfg.power.wol = Some(tandem_power::WolTarget::new(
            "aa:bb:cc:dd:ee:ff".parse().unwrap(),
        ));
        cfg
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::Side;
    use tandem_link::manager::LinkKind;

    use super::*;

    #[test]
    fn parses_full_config() {
        let cfg = fixtures::desk();
        assert_eq!(cfg.machine, Machine::A);
        assert_eq!(cfg.network.port, 47771);
        assert_eq!(cfg.network.peers.len(), 2);
        assert!(cfg.desk.screens.is_empty());
        assert!(cfg.warnings().is_empty());
    }

    #[test]
    fn parses_the_declared_screens() {
        let cfg = AgentConfig::from_toml(
            r#"
            machine = "b"

            [[screens]]
            machine = "b"
            id = "34"
            x = 0
            y = 0
            width = 3440
            height = 1440

            [[screens]]
            machine = "a"
            id = "laptop"
            x = 0
            y = 0
            width = 1512
            height = 982
            scale = 2.0

            [arrangement]
            a_side = "right"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.desk.screens.len(), 2);
        assert_eq!(cfg.desk.screens_of(Machine::A).count(), 1);
        let arr = cfg.desk.arrangement.unwrap();
        assert_eq!(
            (arr.a_side, arr.align),
            (Side::Right, tandem_core::Align::Center)
        );
        let err = AgentConfig::from_toml(
            "machine = \"b\"\n[[screens]]\nmachine = \"b\"\nid = \"a\"\nx = 0\ny = 0\nwidth = 0\nheight = 1\n",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Desk(_)), "{err}");
    }

    /// Una configuración de v0.1 sigue cargando: `[[monitors]]` se ignora con un aviso.
    #[test]
    fn an_old_config_with_monitors_still_loads_with_a_warning() {
        let old = r#"
            machine = "a"
            desk_rev = 3
            desk_origin = "b"

            [[monitors]]
            id = "m34"
            label = '34"'
            edid_model = "LC34G55T"
            wiring = { pc = "displayport", mac = "hdmi" }
        "#;
        let cfg = AgentConfig::from_toml(old).unwrap();
        assert_eq!(cfg.desk, tandem_core::Config::default());
        assert_eq!(cfg.desk_rev, 3);
        let w = cfg.warnings();
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("[[monitors]]"), "{w:?}");
        // Lo que generaba v0.1 (`monitors = []`) no avisa.
        let generated = AgentConfig::from_toml("machine = \"b\"").unwrap();
        assert!(generated.warnings().is_empty());
        // Y desde el archivo también carga.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        std::fs::write(&path, old).unwrap();
        assert_eq!(AgentConfig::load(&path).unwrap(), cfg);
    }

    #[test]
    fn network_is_optional() {
        let cfg = AgentConfig::from_toml("machine = \"b\"").unwrap();
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
            "machine = \"a\"\n[orchestrator]\npower_on_timeout_secs = 90\nstep_timeout_secs = 5",
        )
        .unwrap();
        let t = cfg.orchestrator.timeouts();
        assert_eq!(t.power_on, Duration::from_secs(90));
        assert_eq!(t.step, Duration::from_secs(5));
    }

    #[test]
    fn rejects_zero_timeout() {
        for field in ["power_on_timeout_secs", "step_timeout_secs"] {
            let err =
                AgentConfig::from_toml(&format!("machine = \"a\"\n[orchestrator]\n{field} = 0"))
                    .unwrap_err();
            assert!(matches!(err, ConfigError::ZeroTimeout), "{err}");
        }
    }

    #[test]
    fn rejects_unknown_orchestrator_field() {
        let err =
            AgentConfig::from_toml("machine = \"a\"\n[orchestrator]\ntimeout = 1").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    /// Sin `machine`, el lado A hasta que la identidad diga otra cosa.
    #[test]
    fn without_machine_it_is_side_a() {
        let cfg = AgentConfig::from_toml("monitors = []").unwrap();
        assert_eq!(cfg.machine, Machine::A);
        assert!(!cfg.rewrite_sides);
    }

    /// Un archivo de v0.2 carga con los lados traducidos: esta máquina es A (hasta saber las
    /// claves) y el puesto dice lo mismo que antes.
    #[test]
    fn an_old_config_with_mac_and_pc_is_translated() {
        let cfg = AgentConfig::from_toml(
            "machine = \"pc\"\ndesk_origin = \"mac\"\n[input]\nserver = \"mac\"\n\
             [[screens]]\nmachine = \"pc\"\nid = \"34\"\nx = 0\ny = 0\nwidth = 10\nheight = 10\n\
             [arrangement]\nmac_side = \"left\"\n",
        )
        .unwrap();
        assert!(cfg.rewrite_sides);
        assert_eq!(cfg.machine, Machine::A);
        assert_eq!(cfg.desk_origin, Some(Machine::B));
        assert_eq!(cfg.input.keyboard, Some(Machine::B));
        assert_eq!(cfg.desk.screens[0].machine, Machine::A);
        assert_eq!(cfg.desk.arrangement.unwrap().a_side, Side::Right);
        // Si por las claves esta máquina (la PC) es B, todo se da vuelta y sigue diciendo
        // lo mismo: la Mac (ahora A) a la izquierda.
        let b = cfg.with_side(Machine::B);
        assert_eq!(b.machine, Machine::B);
        assert_eq!(b.desk.screens[0].machine, Machine::B);
        assert_eq!(b.input.keyboard, Some(Machine::A));
        assert_eq!(b.desk_origin, Some(Machine::A));
        assert_eq!(b.desk.arrangement.unwrap().a_side, Side::Left);
        assert!(b.rewrite_sides);
    }

    #[test]
    fn the_same_side_changes_nothing() {
        let cfg = fixtures::desk();
        assert_eq!(cfg.clone().with_side(Machine::A), cfg);
    }

    #[test]
    fn rejects_unknown_machine() {
        let err = AgentConfig::from_toml("machine = \"linux\"").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn rejects_port_zero() {
        let err = AgentConfig::from_toml("machine = \"a\"\n[network]\nport = 0").unwrap_err();
        assert!(matches!(err, ConfigError::ZeroPort));
    }

    #[test]
    fn pairing_port_defaults_to_the_next_one() {
        let cfg = |net: &str| AgentConfig::from_toml(&format!("machine = \"b\"\n[network]\n{net}"));
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
            "machine = \"a\"\n[network]\npeers = [{ kind = \"usb4\", addr = \"10.0.0.1:0\" }]",
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
        let err =
            AgentConfig::from_toml("machine = \"a\"\n[network]\npeers = [\"10.0.0.1:47770\"]")
                .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn rejects_duplicate_peer_link() {
        let err = AgentConfig::from_toml(
            r#"
            machine = "a"
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
        let err = AgentConfig::from_toml("machine = \"a\"\n[network]\nprot = 1").unwrap_err();
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
        let base = "machine = \"a\"\n[power]\n";
        let cfg = AgentConfig::from_toml(&format!("{base}peer_lost_after_secs = 40")).unwrap();
        assert_eq!(cfg.power.peer_lost_after(), Duration::from_secs(40));
        let err = AgentConfig::from_toml(&format!("{base}peer_lost_after_secs = 0")).unwrap_err();
        assert!(matches!(err, ConfigError::ZeroPeerLostAfter), "{err}");
    }

    #[test]
    fn parses_wake_on_lan() {
        let cfg = AgentConfig::from_toml(
            "machine = \"a\"\n[power.wol]\nmac = \"aa-bb-cc-dd-ee-ff\"\nsource = \"10.77.2.1\"\nbroadcast = \"10.77.2.3\"",
        )
        .unwrap();
        let wol = cfg.power.wol.unwrap();
        assert_eq!(wol.mac.to_string(), "aa:bb:cc:dd:ee:ff");
        assert_eq!(wol.destination(), "10.77.2.3:9".parse().unwrap());
    }

    #[test]
    fn rejects_bad_wake_on_lan() {
        let base = "machine = \"a\"\n[power.wol]\nmac = \"aa:bb:cc:dd:ee:ff\"\n";
        let err = AgentConfig::from_toml(&format!("{base}port = 0")).unwrap_err();
        assert!(matches!(err, ConfigError::ZeroWolPort), "{err}");
        let err = AgentConfig::from_toml(&format!("{base}source = \"fe80::1\"")).unwrap_err();
        assert!(
            matches!(err, ConfigError::WolFamilyMismatch { .. }),
            "{err}"
        );
        let err = AgentConfig::from_toml("machine = \"a\"\n[power.wol]\nmac = \"no-es-una-mac\"")
            .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
        let err = AgentConfig::from_toml("machine = \"a\"\n[power]\nwake = 1").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    }

    #[test]
    fn input_defaults() {
        let cfg = fixtures::desk();
        assert_eq!(cfg.input, InputConfig::default());
        assert_eq!(cfg.input.keyboard(), Machine::A);
        assert!(
            !cfg.input.keyboard_defined(),
            "sin definir: no se supone la Mac"
        );
        assert!(cfg.desk.modules.b.is_empty() && cfg.input.hub.is_none());
        assert_eq!(
            shortcut_keys(&cfg.desk.shortcuts.lock).unwrap(),
            [HidUsage::key(0x47)]
        );
        assert_eq!(cfg.input.remap, RemapPolicy::default());
        assert!(!cfg.legacy_desk);
    }

    #[test]
    fn parses_input() {
        let cfg = AgentConfig::from_toml(
            r#"
            machine = "b"
            [input]
            keyboard = "b"
            lock_shortcut = ["ControlLeft", "KeyL"]
            clipboard = false
            [input.hub]
            keyboard = "046d:c33f"
            mouse = "046d:c539"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.input.keyboard(), Machine::B);
        assert!(cfg.input.keyboard_defined());
        // El nombre de antes sigue valiendo.
        let old = AgentConfig::from_toml(
            "machine = \"a\"\n[input]\nserver = \"b\"\n[power]\npc_lost_after_secs = 9",
        )
        .unwrap();
        assert_eq!(old.input.keyboard(), Machine::B);
        assert_eq!(old.power.peer_lost_after_secs, 9);
        // El atajo y el portapapeles de antes pasan al puesto.
        assert!(cfg.legacy_desk);
        assert_eq!(cfg.desk.modules.get(Machine::B, "clipboard"), Some(false));
        assert_eq!(
            shortcut_keys(&cfg.desk.shortcuts.lock).unwrap(),
            [HidUsage::key(0xE0), HidUsage::key(0x0F)]
        );
        let hub = cfg.input.hub.unwrap();
        assert_eq!(hub.keyboard.to_string(), "046d:c33f");
        let none = AgentConfig::from_toml("machine = \"a\"\n[input]\nlock_shortcut = []").unwrap();
        assert!(none.desk.shortcuts.lock.is_empty());
        let new = AgentConfig::from_toml(
            "machine = \"a\"\n[shortcuts]\nlock = [\"KeyL\"]\n[prefs]\ncmd_ctrl = false\n",
        )
        .unwrap();
        assert_eq!(new.desk.shortcuts.lock, ["KeyL"]);
        assert!(!new.desk.prefs.cmd_ctrl && new.desk.prefs.drag_files);
        assert!(!new.legacy_desk);
    }

    #[test]
    fn rejects_bad_input() {
        let base = "machine = \"a\"\n[input]\n";
        let err = AgentConfig::from_toml(&format!("{base}lock_shortcut = [\"Bloq\"]")).unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownKey(ref k) if k == "Bloq"),
            "{err}"
        );
        let err = AgentConfig::from_toml("[shortcuts]\nlock = [\"Bloq\"]").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownKey(_)), "{err}");
        let err = AgentConfig::from_toml(&format!("{base}sever = \"b\"")).unwrap_err();
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
