//! `tandem.toml`: the shared desk ([`tandem_core::Config`]), the network and which side
//! this machine is.
//!
//! ```toml
//! # This machine's side: decided by the public key when pairing (the lower is `a`).
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
//! # Without links to the other machine during this, it is taken as off (both watch).
//! peer_lost_after_secs = 15
//! # This machine is woken by Wake-on-LAN (it announces it to the other).
//! wakeable = true
//!
//! # How to wake the other machine: Wake-on-LAN over the direct Ethernet link.
//! [power.wol]
//! mac = "aa:bb:cc:dd:ee:ff"
//! source = "10.77.2.1"
//! broadcast = "10.77.2.3"
//!
//! [input]
//! # Where the keyboard and mouse are at startup, until the hub decides.
//! keyboard = "a"
//!
//! # The hub's keyboard and mouse, by VID:PID. Without this section there are no automatic roles.
//! [input.hub]
//! keyboard = "046d:c33f"
//! mouse = "046d:c539"
//!
//! # Which machine each side is: filled in by the agent with what each announces.
//! [machines.a]
//! os = "macos"
//! name = "MacBook Pro"
//! color = "#7aa7e6"
//!
//! # The screen arrangement (DESIGN §7). Without `[[screens]]` for a machine, the ones
//! # it reads itself apply; without `[arrangement]`, A to the left of B.
//! [[screens]]
//! machine = "b"
//! id = "34"
//! x = 0
//! y = 0
//! width = 3440
//! height = 1440
//! scale = 1.0
//!
//! # Which side of B A is on (left, right, up, down) and the alignment (start, center, end).
//! [arrangement]
//! a_side = "left"
//! align = "center"
//!
//! # Locks the cursor on the machine it is on (`KeyboardEvent.code` names).
//! [shortcuts]
//! lock = ["ScrollLock"]
//!
//! [prefs]
//! cmd_ctrl = true
//! drag_files = true
//!
//! # Modules the user turned off on each machine (the others, on).
//! [modules.b]
//! clipboard = false
//! ```
//!
//! Everything that is not this machine's (`[machines]`, `[[screens]]`, `[arrangement]`,
//! `[[scenes]]`, `[shortcuts]`, `[prefs]`, `[modules]`) is the shared desk: it is edited
//! from the UI (D4) and written by the agent.
//!
//! A v0.1 configuration with `[[monitors]]` still loads: the section is ignored with
//! a warning in the log ([`AgentConfig::warnings`]). A v0.2 one, with the `mac`/`pc` sides,
//! is translated on its own ([`crate::migrate`]), and so is one from before T-099 (shortcut and clipboard in
//! `[input]`, offset in `[arrangement]`).

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

/// Default UDP port of the control plane (QUIC). Outside 47000-48999: Windows
/// reserves that range on PCs with WSL or Hyper-V (the former 47770 gave `os error 10048`).
pub const DEFAULT_PORT: u16 = 52000;

/// The ones tried, in order, if the configured one cannot be opened
/// ([`crate::network::bind_first`]). Spread over several blocks, because Windows reserves
/// ports in ranges, and all outside 47000-48999. Each one leaves the next free
/// for pairing.
pub const FALLBACK_PORTS: [u16; 8] = [52000, 52100, 53100, 54200, 55300, 56400, 45500, 44600];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    /// This machine's side. Decided by the identity ([`AgentConfig::with_side`]); what is
    /// saved is the last thing resolved, and without a peer yet it is `A`.
    #[serde(default = "default_side")]
    pub machine: Machine,
    /// This machine's system. It is not saved: it comes from the binary ([`Os::local`]); tests
    /// and the simulation change it to build desks of any combination.
    #[serde(skip, default = "Os::local")]
    pub os: Os,
    /// The file's sides are not the ones that hold: it had the v0.2 ones (`mac`/`pc`,
    /// [`crate::migrate`]) or the identity said this machine is the other side. The agent
    /// rewrites it once it knows the file.
    #[serde(skip)]
    pub rewrite_sides: bool,
    /// The desk came in the pre-T-099 format ([`crate::migrate::desk_v03`]): its
    /// fingerprint no longer says anything and the agent rewrites it once it knows the file, without counting it
    /// as a change.
    #[serde(skip)]
    pub legacy_desk: bool,
    /// The migration moved into the desk something that belonged to this machine (the turned-off clipboard) or
    /// gave it the default scenes (T-100): it counts as a change by this machine, with a new
    /// version, so the other adopts it.
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
    /// Version of the shared desk, which syncs with the other machine
    /// ([`tandem_core::DeskVersion`]). Written by the agent; 0 = never changed since it was
    /// generated.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub desk_rev: u64,
    /// When it was made (milliseconds since the Unix epoch, the clock of whoever made it).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub desk_at: u64,
    /// Which machine made the last change of `desk_rev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desk_origin: Option<Machine>,
    /// The fingerprint of the desk the agent wrote with `desk_rev`
    /// ([`crate::desk_sync::fingerprint`]). If it does not match, someone edited it by hand: it is a
    /// change by this machine and is sent to the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desk_sum: Option<String>,
    /// How many v0.1 `[[monitors]]` it has: read only to warn that it is no longer used.
    #[serde(
        default,
        rename = "monitors",
        deserialize_with = "count_entries",
        skip_serializing
    )]
    pub legacy_monitors: Option<usize>,
    /// The desk: it is the same on both machines, except `machine`.
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
    /// Fixed addresses of the other agent, one per link (the direct ones:
    /// USB4 and Ethernet). The LAN one is discovered over mDNS if not fixed.
    #[serde(default)]
    pub peers: Vec<LinkCandidate>,
    /// UDP port the PC waits on while pairing (and the one the Mac asks it to
    /// pair over the fixed links). By default, the one after `port`.
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

/// Maximum times of a transaction's steps (power, monitors, scenes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorConfig {
    /// Powering on the PC and waiting for its agent to answer.
    #[serde(default = "default_power_on_timeout")]
    pub power_on_timeout_secs: u64,
    /// Any other step.
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

/// Power on and off of both machines (DESIGN §3.7: either can be the main one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PowerRaw")]
pub struct PowerConfig {
    /// How long without any link to the other machine makes this one consider it
    /// off (a power cut, or it suspended). Shorter than a reboot is not
    /// of much use, but any brief cable drop has to stay
    /// below it. `pc_lost_after_secs` is the former name.
    #[serde(default = "default_peer_lost_after", alias = "pc_lost_after_secs")]
    pub peer_lost_after_secs: u64,
    /// This machine is woken by Wake-on-LAN; it is announced to the other.
    #[serde(default = "default_true")]
    pub wakeable: bool,
    /// How to wake the other machine. Without this section, powering it on fails with an error
    /// clear error.
    #[serde(default)]
    pub wol: Option<WolTarget>,
    /// The user deleted the other's MAC (`[power.wol] cleared = true`): it is not learned
    /// again on its own when pairing; writing one by hand removes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wol_cleared: bool,
    /// The MAC of this machine's Ethernet card, announced to the other so that it
    /// saves it when pairing (T-159). Without it, the agent detects it on start
    /// ([`tandem_power::nic::detect`]); writing it fixes the one that is announced.
    #[serde(default)]
    pub mac: Option<tandem_power::MacAddr>,
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
            wol_cleared: false,
            mac: None,
        }
    }
}

/// What the file says; `[power.wol]` can be just `cleared = true`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PowerRaw {
    #[serde(default = "default_peer_lost_after", alias = "pc_lost_after_secs")]
    peer_lost_after_secs: u64,
    #[serde(default = "default_true")]
    wakeable: bool,
    #[serde(default)]
    wol: Option<WolRaw>,
    #[serde(default)]
    mac: Option<tandem_power::MacAddr>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WolRaw {
    #[serde(default)]
    mac: Option<tandem_power::MacAddr>,
    #[serde(default)]
    cleared: bool,
    #[serde(default)]
    source: Option<std::net::IpAddr>,
    #[serde(default)]
    broadcast: Option<std::net::IpAddr>,
    #[serde(default)]
    port: Option<u16>,
}

impl TryFrom<PowerRaw> for PowerConfig {
    type Error = String;

    fn try_from(raw: PowerRaw) -> Result<Self, String> {
        let (wol, wol_cleared) = match raw.wol {
            None => (None, false),
            // A written MAC wins over `cleared`.
            Some(w) => match w.mac {
                Some(mac) => {
                    let mut t = WolTarget::new(mac);
                    t.source = w.source;
                    if let Some(b) = w.broadcast {
                        t.broadcast = b;
                    }
                    if let Some(p) = w.port {
                        t.port = p;
                    }
                    (Some(t), false)
                }
                None if w.cleared => (None, true),
                None => return Err("[power.wol] needs `mac` (or `cleared = true`)".into()),
            },
        };
        Ok(PowerConfig {
            peer_lost_after_secs: raw.peer_lost_after_secs,
            wakeable: raw.wakeable,
            wol,
            wol_cleared,
            mac: raw.mac,
        })
    }
}

impl PowerConfig {
    pub fn peer_lost_after(&self) -> Duration {
        Duration::from_secs(self.peer_lost_after_secs)
    }
}

/// Keyboard and mouse of this machine (DESIGN §7). What is shared is in the desk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Where the keyboard and mouse are at startup, until the hub (or the other agent) says
    /// otherwise. `server` is the former name.
    /// Without it (the configuration T-087 generates does not write it) it is not known: the engine starts
    /// with side A, but the UI says "not set" until the hub or setup
    /// say so ([`InputConfig::keyboard`], [`InputConfig::keyboard_defined`]).
    #[serde(default, alias = "server", skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<Machine>,
    /// The hub's keyboard and mouse. Without this the roles do not change on their own.
    #[serde(default)]
    pub hub: Option<HubConfig>,
    /// The table for each direction of Cmd<->Ctrl; whether it is remapped or not is stated by the desk
    /// (`[prefs] cmd_ctrl`).
    #[serde(default)]
    pub remap: RemapPolicy,
    /// Whether a drag (a mouse button pressed) crosses to the other machine. By
    /// default no, like Deskflow (T-093).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drag_between_machines: bool,
}

/// Both machines have to start with the same server even if neither knows anything: side
/// A, which both know without talking.
fn default_keyboard() -> Machine {
    Machine::A
}

impl InputConfig {
    /// Where the input engine starts: the configured one or, if unset, side A. For the
    /// UI, see [`InputConfig::keyboard_defined`].
    pub fn keyboard(&self) -> Machine {
        self.keyboard.unwrap_or_else(default_keyboard)
    }

    /// Whether the keyboard is stated in the configuration.
    pub fn keyboard_defined(&self) -> bool {
        self.keyboard.is_some()
    }
}

fn default_true() -> bool {
    true
}

/// The keys of a desk shortcut (`[shortcuts]`), by their `KeyboardEvent.code` name.
pub fn shortcut_keys(names: &[String]) -> Result<Vec<HidUsage>, ConfigError> {
    names
        .iter()
        .map(|name| from_code_name(name).ok_or_else(|| ConfigError::UnknownKey(name.clone())))
        .collect()
}

/// All the keys of the desk's shortcuts (the lock one and the scenes') exist.
pub fn desk_keys(desk: &tandem_core::Desk) -> Result<(), ConfigError> {
    shortcut_keys(&desk.shortcuts.lock)?;
    for scene in &desk.scenes {
        shortcut_keys(&scene.shortcut)?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid configuration: {0}")]
    Parse(#[from] toml::de::Error),
    #[error(transparent)]
    Desk(#[from] tandem_core::model::ConfigError),
    #[error("the network port cannot be 0")]
    ZeroPort,
    #[error("the pairing port ({0}) cannot be 0 nor the same as the network's")]
    BadPairingPort(u16),
    #[error("the steps' maximum times cannot be 0")]
    ZeroTimeout,
    #[error(
        "power.peer_lost_after_secs cannot be 0: any blink of the links would make the other machine considered off"
    )]
    ZeroPeerLostAfter,
    #[error("the Wake-on-LAN port cannot be 0")]
    ZeroWolPort,
    #[error(
        "Wake-on-LAN: the source {source_ip} and the destination {broadcast} are not of the same family (IPv4/IPv6)"
    )]
    WolFamilyMismatch {
        source_ip: std::net::IpAddr,
        broadcast: std::net::IpAddr,
    },
    #[error(transparent)]
    Peers(#[from] DiscoveryError),
    #[error(
        "shortcuts.lock: \"{0}\" is not a key (KeyboardEvent.code names are used, like ScrollLock or KeyL)"
    )]
    UnknownKey(String),
}

impl AgentConfig {
    /// Reads and validates. A v0.2 file (`mac`/`pc` sides) is translated first, with this
    /// machine as `A` until the identity says otherwise ([`crate::migrate`]).
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        let mut table: toml::Table = s.parse()?;
        // First the desk: it decides whether the arrangement was the default one with the names
        // the file carries, before translating them.
        let migrated = crate::migrate::desk_v03(&mut table);
        let legacy = crate::migrate::sides_v02(&mut table);
        // A pre-T-100 desk does not have the `scenes` key (one without scenes on purpose
        // has `scenes = []`).
        let before_scenes = !table.contains_key("scenes");
        let mut cfg: AgentConfig = table.try_into()?;
        cfg.rewrite_sides = legacy;
        // Spanish ids from before T-209: translated here and written back once, like the
        // pre-T-099 desk (the fingerprint of the old one no longer matches).
        let renamed = cfg.desk.migrate_legacy_ids();
        cfg.legacy_desk = migrated.changed || renamed;
        cfg.local_to_desk = migrated.local;
        // Already paired: the default scenes replace the modes (D6). It is a change by
        // this machine, with a new version: it is saved and reaches the other.
        if before_scenes && tandem_core::scene::add_defaults(&mut cfg.desk) {
            cfg.local_to_desk = true;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    /// The same configuration seen from side `side`: if it is not the saved one, the sides
    /// of everything that names them are switched (the desk, the keyboard, the author of the last
    /// change). Nothing physical changes: each screen is still its machine's.
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

    /// Reads `path` and logs what is ignored ([`AgentConfig::warnings`]).
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

    /// What the configuration carries and is no longer used. An empty `monitors = []` (what
    /// v0.1 generated) bothers nobody and is not warned about.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.legacy_monitors.is_some_and(|n| n > 0) {
            out.push(
                "the [[monitors]] section is no longer used (v0.2 removed monitor control): it is ignored"
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
    /// Candidates for the link manager: the config's fixed ones plus what
    /// is discovered on the LAN (`discovered`, from the best to the worst address).
    pub fn link_candidates(&self, discovered: &[std::net::SocketAddr]) -> Vec<LinkCandidate> {
        link_candidates(&self.peers, discovered)
    }

    /// See [`NetworkConfig::pairing_port`](field@NetworkConfig::pairing_port).
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

    /// The desk with `[power.wol]`: this machine can wake the other.
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

    /// A v0.1 configuration still loads: `[[monitors]]` is ignored with a warning.
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
        // What v0.1 generated (`monitors = []`) does not warn.
        let generated = AgentConfig::from_toml("machine = \"b\"").unwrap();
        assert!(generated.warnings().is_empty());
        // And from the file it also loads.
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

    /// Without `machine`, side A until the identity says otherwise.
    #[test]
    fn without_machine_it_is_side_a() {
        let cfg = AgentConfig::from_toml("monitors = []").unwrap();
        assert_eq!(cfg.machine, Machine::A);
        assert!(!cfg.rewrite_sides);
    }

    /// A v0.2 file loads with the sides translated: this machine is A (until the
    /// keys are known) and the desk says the same as before.
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
        // If by the keys this machine (the PC) is B, everything is flipped and keeps saying
        // the same: the Mac (now A) on the left.
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
    fn a_cleared_wake_on_lan_is_remembered() {
        let cfg = AgentConfig::from_toml("machine = \"a\"\n[power.wol]\ncleared = true").unwrap();
        assert_eq!(cfg.power.wol, None);
        assert!(cfg.power.wol_cleared);
        // Without MAC and without `cleared` it is not valid; with a MAC, the MAC wins.
        assert!(AgentConfig::from_toml("machine = \"a\"\n[power.wol]\nport = 9").is_err());
        let cfg = AgentConfig::from_toml(
            "machine = \"a\"\n[power.wol]\ncleared = true\nmac = \"aa:bb:cc:dd:ee:ff\"",
        )
        .unwrap();
        assert!(cfg.power.wol.is_some() && !cfg.power.wol_cleared);
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
        let err = AgentConfig::from_toml("machine = \"a\"\n[power.wol]\nmac = \"not-a-mac\"")
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
            "not set: the Mac is not assumed"
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
        // The former name still holds.
        let old = AgentConfig::from_toml(
            "machine = \"a\"\n[input]\nserver = \"b\"\n[power]\npc_lost_after_secs = 9",
        )
        .unwrap();
        assert_eq!(old.input.keyboard(), Machine::B);
        assert_eq!(old.power.peer_lost_after_secs, 9);
        // The shortcut and clipboard from before move to the desk.
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

    /// T-209: the Spanish ids of an old file are mapped when reading; a new file is not touched.
    #[test]
    fn the_spanish_ids_of_an_old_file_are_mapped() {
        let old = "machine = \"a\"\n[[scenes]]\nid = \"trabajo\"\nname = \"Trabajo\"\nicon = \"puesto\"\n\
                   [[screens]]\nmachine = \"a\"\nid = \"pantalla-1\"\nx = 0\ny = 0\nwidth = 10\nheight = 10\n";
        let cfg = AgentConfig::from_toml(old).unwrap();
        assert!(cfg.legacy_desk);
        assert_eq!(cfg.desk.scenes[0].id, "work");
        assert_eq!(cfg.desk.scenes[0].name, "Work");
        assert_eq!(cfg.desk.scenes[0].icon, "desk");
        assert_eq!(cfg.desk.screens[0].id, "screen-1");

        let new = "machine = \"a\"\n[[scenes]]\nid = \"work\"\nname = \"Work\"\nicon = \"desk\"\n";
        let cfg = AgentConfig::from_toml(new).unwrap();
        assert!(!cfg.legacy_desk);
        assert_eq!(cfg.desk.scenes[0].id, "work");
    }

    #[test]
    fn rejects_bad_input() {
        let base = "machine = \"a\"\n[input]\n";
        let err =
            AgentConfig::from_toml(&format!("{base}lock_shortcut = [\"NotAKey\"]")).unwrap_err();
        assert!(
            matches!(err, ConfigError::UnknownKey(ref k) if k == "NotAKey"),
            "{err}"
        );
        let err = AgentConfig::from_toml("[shortcuts]\nlock = [\"NotAKey\"]").unwrap_err();
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
        let err = AgentConfig::load(&dir.path().join("missing.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }));
    }
}
