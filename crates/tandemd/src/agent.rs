//! The agent's live state and what it answers to the UI.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::plan::PlanError;
use tandem_core::{
    Abilities, Config, DeskHistory, DeskPatch, DeskStamp, DeskVersion, HistoryEntry, Machine,
    MachineInfo, Plan, State,
};
use tandem_link::qos::LinkQos;
use tandem_link::transport::{Connection, ControlStream};
use tandem_proto::{
    Capabilities, Channel, ControlMsg, DeskCapabilities, FocusMsg, HubReport, InputStatus,
    KeyProbeStatus, ModuleId, PairingStatus, PeerStatus, PowerEvent, Reply, ScreenRect, SetupInfo,
    TxnEvent, UiCommand, UpdateMsg, UpdateStatus,
};
use tokio::sync::{broadcast, mpsc, watch};

mod cursor;
mod env;
mod modules;
mod monitors;
mod net;
mod scenes;
mod setup;
mod update;
mod wol;

pub use cursor::CursorWatch;
pub use env::ResourceWatch;
pub use scenes::{SceneBackend, SceneShortcuts, UNKNOWN_SHOWS};

use crate::capabilities;
use crate::config::AgentConfig;
use crate::modules::{ModuleContext, ModuleSwitches, Registry};
use crate::monitors::MonitorHub;
use crate::orchestrator::{LocalSteps, Orchestrator, PeerLink, Unimplemented};
use crate::store::{StateStore, StoreError};

/// What the UI receives if it asks for a change while another is in progress.
pub const BUSY: &str = "a change is already in progress";
/// What the UI receives if it asks to pair without the network running.
pub const NO_NETWORK: &str = "the agent's network is not running";
/// The Tandem version this agent runs: the app and the agent ship in the same package.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whoever knows how to pair (the network): the agent hands it the UI's requests and publishes
/// progress with [`Agent::set_pairing_status`]. Each method answers right away; what
/// takes time continues in the background. `Err` is the reason for the UI.
pub trait PairingControl: Send + Sync {
    fn start(&self) -> Result<(), String>;
    fn confirm(&self) -> Result<(), String>;
    fn cancel(&self) -> Result<(), String>;
    fn unpair(&self) -> Result<(), String>;
    /// The other machine left the desk and said so (T-152): it is forgotten just the same, but without
    /// notifying it again. A notice that arrives without a peer is not an error.
    fn peer_left(&self) -> Result<(), String> {
        self.unpair()
    }
}

#[derive(Debug)]
pub struct Agent {
    cfg: AgentConfig,
    /// Whom this machine wakes now (T-159): the `[power.wol]` of `cfg`, the one learned
    /// when pairing or the one the UI wrote.
    wol: std::sync::RwLock<Option<tandem_power::WolTarget>>,
    /// The user deleted the MAC: `learn_wol` does not learn it again (`[power.wol] cleared`).
    wol_cleared: std::sync::atomic::AtomicBool,
    /// The tunnels this machine hosts and the ones it opened (T-103, [`crate::tunnels`]).
    tunnels: tandem_tunnel::Tunnels,
    /// Files and ports per tunnel (T-106): the destination side and the local clients.
    files: tandem_files::Files,
    workspaces: tandem_workspace::Workspaces,
    /// Priority, cap and pause of the data, one per path (T-101).
    link_qos: LinkQos,
    /// This machine's side. It comes from the identity: it changes only once, if when pairing
    /// it turns out this machine is the other side ([`Agent::adopt_side`]).
    me: watch::Sender<Machine>,
    store: StateStore,
    state: watch::Sender<State>,
    /// Reading the state and saving it must be a single operation: otherwise, a
    /// save with the old state may end after one with the new.
    persisting: Mutex<()>,
    orch: Orchestrator,
    /// Power notices about the other machine that do not come from a transaction.
    power: broadcast::Sender<PowerEvent>,
    /// Whom it is paired with and over which link it sees them; updated by the network.
    peer_status: watch::Sender<PeerStatus>,
    /// How pairing is going; updated by the network.
    pairing_status: watch::Sender<PairingStatus>,
    pairing: Mutex<Option<Arc<dyn PairingControl>>>,
    /// The keyboard and mouse as the UI sees them; updated by [`crate::input`].
    input_status: watch::Sender<InputStatus>,
    /// What the other agent sends for the keyboard and mouse.
    inbox: PeerInbox,
    drag: crate::drag::DragHub,
    /// The shared desk that holds now (the one from `tandem.toml` or the one that came from the other).
    desk: watch::Sender<Config>,
    /// `tandem.toml`, where what changes is saved; without it, only in memory.
    config_file: Mutex<Option<PathBuf>>,
    /// Where the configuration came from and which port the agent listens on.
    setup: Mutex<SetupInfo>,
    /// What this machine can do and the latest the other announced (T-084).
    caps: watch::Sender<DeskCapabilities>,
    /// The modules that exist on this machine (T-098).
    registry: Registry,
    /// The modules turned off here (copy of this side's `Desk.modules`, which is the source) and
    /// the problems each one running reported.
    modules: watch::Sender<ModuleSwitches>,
    /// Changes to what this machine announces go to the other together (T-098).
    announce: capabilities::Debounce,
    /// The stamp of the desk that holds now and the latest versions ([`crate::desk_sync`]).
    sync: Mutex<SyncClock>,
    /// Notices that the other machine changed the desk (D5), for the UI.
    desk_changes: broadcast::Sender<DeskChanged>,
    /// The `tandem.toml` desk was edited by hand: once the file is known the new
    /// version is written to it.
    hand_edited: bool,
    /// The file's sides are not the ones that hold (v0.2, or the identity said otherwise): once
    /// the file is known they are rewritten whole.
    rewrite_sides: bool,
    /// One desk change (and of `tandem.toml`) at a time: switching sides while the other's
    /// desk arrives cannot mix old and new labels either in memory or in the
    /// file.
    editing: Mutex<()>,
    /// The setup's "press a key" test.
    key_probe: watch::Sender<KeyProbeStatus>,
    /// There is an input engine that builds the [`InputStatus`] (its state task listens to what
    /// the user declares); without it, the agent updates it directly.
    input_engine: std::sync::atomic::AtomicBool,
    /// Where the keyboard is according to the configuration or setup; `None`, unset.
    keyboard: watch::Sender<Option<Machine>>,
    /// How many times the user asked for the macOS permissions (T-090): the input engine
    /// listens to the counter and asks the system for them.
    permission_requests: watch::Sender<u64>,
    /// "Lock here" requests (T-129): the counter goes up with each one and the input
    /// engine applies the last requested value.
    lock_requests: watch::Sender<(u64, bool)>,
    /// This process, to announce it in the capabilities.
    process: tandem_proto::ProcessInfo,
    /// The desk's monitors: channels, owners and what each owner read (T-110).
    monitors: Arc<MonitorHub>,
    /// Detected environments and live resources (T-105).
    env: env::EnvState,
    /// The versions of both machines and the request to update this one (T-122).
    update: watch::Sender<UpdateStatus>,
    /// A double of the monitor and geometry scene steps (T-100's simulation);
    /// `None`: the real monitors ([`scenes`], T-111).
    scene_backend: std::sync::RwLock<Option<Arc<dyn SceneBackend>>>,
    /// Network metrics and the connection of each healthy path (T-102).
    net: net::NetState,
    /// The live cursor, while someone is watching it (T-128).
    cursor: cursor::CursorState,
}

#[derive(Debug, Clone)]
struct SyncClock {
    stamp: DeskStamp,
    history: DeskHistory,
}

/// The other machine changed the desk and its version won: what the "Changed from
/// X · Undo" notice (`AgentToUi::DeskChanged`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskChanged {
    pub by: Machine,
    pub rev: u64,
    pub summary: String,
    /// The version that was here before.
    pub undo_to: u64,
}

/// Milliseconds since the Unix epoch: the date of a desk change.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl std::fmt::Debug for dyn PairingControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingControl")
    }
}

/// The latest the other agent sent for the keyboard and mouse, and whom to hand the
/// input and clipboard streams it opens to.
#[derive(Debug)]
pub struct PeerInbox {
    pub hub: watch::Sender<Option<HubReport>>,
    pub screens: watch::Sender<Option<Vec<ScreenRect>>>,
    channels: Mutex<Option<mpsc::UnboundedSender<IncomingChannel>>>,
    /// Whoever arbitrates the keyboard and mouse (D7): receives the other's [`FocusMsg`]s.
    focus: Mutex<Option<mpsc::UnboundedSender<FocusMsg>>>,
}

/// An input or clipboard stream the other agent opened, over its connection
/// (the mouse datagrams arrive over the same one).
pub struct IncomingChannel {
    pub channel: Channel,
    pub conn: Connection,
    pub stream: ControlStream,
}

impl std::fmt::Debug for IncomingChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingChannel")
            .field("channel", &self.channel)
            .field("from", &self.conn.remote_address())
            .finish_non_exhaustive()
    }
}

impl PeerInbox {
    fn new() -> Self {
        PeerInbox {
            hub: watch::Sender::new(None),
            screens: watch::Sender::new(None),
            channels: Mutex::new(None),
            focus: Mutex::new(None),
        }
    }

    /// Whoever handles the keyboard and mouse arbitration; replaces the previous one.
    pub fn set_focus(&self, tx: mpsc::UnboundedSender<FocusMsg>) {
        *self.focus.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    }

    /// Delivers an arbitration message; without an input engine it is discarded.
    pub fn deliver_focus(&self, msg: FocusMsg) {
        let tx = self.focus.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(tx) = tx.as_ref()
            && tx.send(msg).is_err()
        {
            tracing::debug!("keyboard arbitration with nobody to handle it");
        }
    }

    /// Whoever handles the input and clipboard streams; replaces the previous one.
    pub fn set_channels(&self, tx: mpsc::UnboundedSender<IncomingChannel>) {
        *self.channels.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    }

    /// Delivers the stream; with nobody to handle it, it is discarded (closed).
    pub fn deliver(&self, incoming: IncomingChannel) {
        let tx = self.channels.lock().unwrap_or_else(PoisonError::into_inner);
        match tx.as_ref() {
            Some(tx) => {
                if let Err(e) = tx.send(incoming) {
                    tracing::debug!(channel = ?e.0.channel, "stream from the other agent with nobody to handle it");
                }
            }
            None => tracing::debug!(
                canal = ?incoming.channel,
                "stream from the other agent without an input engine: closing it"
            ),
        }
    }
}

/// Power notices kept for a slow subscriber.
const POWER_EVENT_BUFFER: usize = 16;
/// Desk change notices kept for a slow subscriber.
const DESK_EVENT_BUFFER: usize = 16;

impl Agent {
    /// Starts from the saved state (or the initial one, if there is none), without platform
    /// backends: every local step answers "not implemented".
    pub fn new(cfg: AgentConfig, store: StateStore) -> Self {
        Self::with_local_steps(cfg, store, Arc::new(Unimplemented))
    }

    /// With this machine's real backends ([`crate::PowerSteps::for_machine`] and the
    /// environment scan of [`tandem_env::Host::system`]).
    pub fn with_platform(cfg: AgentConfig, store: StateStore) -> Self {
        let mut cfg = cfg;
        // This machine's MAC, so the other saves it when pairing (T-159).
        if cfg.power.mac.is_none() {
            cfg.power.mac = tandem_power::nic::detect();
        }
        let local = Arc::new(crate::power::PowerSteps::for_machine(&cfg));
        Self::with_local_steps(cfg, store, local)
            .with_env_host(Arc::new(tandem_env::Host::system()))
    }

    /// Like [`Agent::new`], with the steps this machine knows how to do.
    pub fn with_local_steps(
        cfg: AgentConfig,
        store: StateStore,
        local: Arc<dyn LocalSteps>,
    ) -> Self {
        let me = cfg.machine;
        let mut state = store.load_following(me, &cfg.legacy_renames);
        // This machine is always powered on for itself (DESIGN §3.7); the other, until
        // the links say otherwise, as it was left.
        state.on.set(me, true);
        let orch = Orchestrator::new(me, local, cfg.orchestrator.timeouts());
        let monitors = Arc::new(MonitorHub::new(me));
        orch.set_monitors(monitors.clone());
        let mut input = InputStatus::new(cfg.input.keyboard());
        input.defined = cfg.input.keyboard_defined();
        let process = capabilities::current_process();
        // A desk edited by hand in `tandem.toml` is a change by this machine: new
        // version, so the other adopts it and does not overwrite it with its own. One in the pre-T-099
        // format has an old fingerprint: it is rewritten without counting it as a change, unless
        // something that belonged to this machine moved into the desk (review of #74).
        let hand_edited = cfg.local_to_desk
            || (!cfg.legacy_desk
                && crate::desk_sync::edited_by_hand(&cfg.desk, cfg.desk_sum.as_deref()));
        let info = local_info(&cfg, me, &cfg.desk);
        let registry = Registry::for_os(cfg.os);
        let mut switches = modules::initial_switches(&cfg);
        // Until it starts with a bus ([`Agent::start_monitors`]).
        switches.set_problem(
            tandem_proto::ModuleId::Monitors,
            Some(crate::monitors::NO_DDC.into()),
        );
        let modules = registry.snapshot(&ModuleContext {
            os: info.os,
            input: &input,
            power: &capabilities::power(&cfg, info.os),
            switches: &switches,
        });
        let here = capabilities::local(&cfg, me, info, &input, &process, modules);
        let saved = DeskStamp {
            rev: cfg.desk_rev,
            at: cfg.desk_at,
            by: cfg.desk_origin.unwrap_or(me),
        };
        let stamp = if hand_edited {
            tracing::info!("the tandem.toml desk changed by hand: sending it to the other machine");
            saved.next(me, now_millis())
        } else {
            saved
        };
        let sync = SyncClock {
            stamp,
            history: DeskHistory::new(DeskVersion {
                stamp,
                desk: cfg.desk.clone(),
            }),
        };
        let link_qos = LinkQos::default();
        link_qos.set_prefs(cfg.desk.data);
        Agent {
            tunnels: tandem_tunnel::Tunnels::default(),
            files: tandem_files::Files::default(),
            // Next to the state: `<data>/workspaces` (T-107).
            workspaces: tandem_workspace::Workspaces::new(crate::paths::absolute(
                store
                    .path()
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("workspaces"),
            )),
            link_qos,
            me: watch::Sender::new(me),
            editing: Mutex::new(()),
            rewrite_sides: cfg.rewrite_sides,
            store,
            state: watch::Sender::new(state),
            persisting: Mutex::new(()),
            orch,
            power: broadcast::channel(POWER_EVENT_BUFFER).0,
            // Until the network says otherwise, the agent works alone.
            peer_status: watch::Sender::new(PeerStatus::Unpaired),
            pairing_status: watch::Sender::new(PairingStatus::Idle),
            pairing: Mutex::new(None),
            input_status: watch::Sender::new(input),
            inbox: PeerInbox::new(),
            drag: crate::drag::DragHub::default(),
            desk: watch::Sender::new(cfg.desk.clone()),
            config_file: Mutex::new(None),
            setup: Mutex::new(SetupInfo {
                config_file: None,
                generated: false,
                port: None,
            }),
            caps: watch::Sender::new(DeskCapabilities { here, peer: None }),
            registry,
            modules: watch::Sender::new(switches),
            announce: capabilities::Debounce::new(capabilities::ANNOUNCE_DEBOUNCE),
            sync: Mutex::new(sync),
            desk_changes: broadcast::channel(DESK_EVENT_BUFFER).0,
            hand_edited,
            key_probe: watch::Sender::new(KeyProbeStatus::default()),
            keyboard: watch::Sender::new(cfg.input.keyboard),
            input_engine: std::sync::atomic::AtomicBool::new(false),
            permission_requests: watch::Sender::new(0),
            lock_requests: watch::Sender::new((0, false)),
            process,
            monitors,
            // Without backends nothing is found; [`Agent::with_env_host`] sets the real one.
            env: env::EnvState::new(Arc::new(tandem_env::Host::none())),
            update: watch::Sender::new(UpdateStatus {
                here: VERSION.to_owned(),
                ..UpdateStatus::default()
            }),
            scene_backend: std::sync::RwLock::new(None),
            net: net::NetState::default(),
            cursor: cursor::CursorState::default(),
            wol: std::sync::RwLock::new(cfg.power.wol.clone()),
            wol_cleared: std::sync::atomic::AtomicBool::new(cfg.power.wol_cleared),
            cfg,
        }
    }

    /// The configuration file, where what changes is saved. If the desk was edited
    /// by hand, the new version is written to it right away.
    pub fn set_config_file(&self, path: PathBuf) {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .config_file = Some(path.display().to_string());
        *self
            .config_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(path);
        let _editing = self.edit();
        if self.rewrite_sides || self.cfg.legacy_desk {
            self.save_sides();
        } else if self.hand_edited {
            self.save_desk(&self.desk_sync());
        }
    }

    fn edit(&self) -> std::sync::MutexGuard<'_, ()> {
        self.editing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Everything that names a side, to the file (see [`crate::config_edit::with_sides`]).
    fn save_sides(&self) {
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_sides(
                &path,
                self.machine(),
                self.keyboard_declared(),
                &self.desk_sync(),
            )
        {
            tracing::warn!("sides were not saved to {}: {e}", path.display());
        }
    }

    /// This machine turned out to be side `side` (the identity of the just-paired peer says
    /// so, DECISIONS A1). If it already was, nothing; if not, the sides of everything that
    /// names them are switched (state, desk, keyboard, capabilities), saved and announced: the input
    /// engine restarts with the new side.
    pub fn adopt_side(&self, side: Machine) {
        let _editing = self.edit();
        if self.machine() == side {
            return;
        }
        tracing::info!(side = %side, "by the keys, this machine is the other side");
        // Stamp and desk change under the same lock that `desk_sync` reads with.
        {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.stamp = clock.stamp.swapped();
            clock.history = std::mem::take(&mut clock.history).swapped();
            let synced = clock.stamp.rev > 0;
            self.desk.send_modify(|d| {
                let arrangement = d.arrangement;
                *d = std::mem::take(d).swap_sides();
                // An arrangement that never synced is not a physical place to preserve: it is the
                // one there was with the new sides. Flipping it would leave both new installations
                // with A on the right. Without a declared arrangement there is nothing to flip.
                if !synced {
                    d.arrangement = arrangement;
                }
            });
        }
        self.record_current();
        self.keyboard.send_modify(|k| *k = k.map(Machine::other));
        self.key_probe
            .send_modify(|p| p.heard = p.heard.map(Machine::other));
        self.input_status
            .send_modify(|s| s.server = s.server.other());
        self.orch.set_machine(side);
        self.monitors.set_me(side);
        self.me.send_replace(side);
        let state = self.state().swapped();
        if let Err(e) = self.set_state(state) {
            tracing::warn!("{e}");
        }
        self.caps.send_modify(|c| c.peer = None);
        self.sync_monitor_peer();
        self.refresh_capabilities();
        self.save_sides();
    }

    /// Notifies if this machine changes side ([`Agent::adopt_side`]).
    pub fn subscribe_machine(&self) -> watch::Receiver<Machine> {
        self.me.subscribe()
    }

    /// Which machine `side` is according to the desk, if already known.
    pub fn machine_info(&self, side: Machine) -> Option<MachineInfo> {
        self.desk.borrow().machines.get(side).cloned()
    }

    /// The name of `side` for texts: the desk's or "machine A".
    pub fn machine_name(&self, side: Machine) -> String {
        self.desk.borrow().machines.name(side)
    }

    /// Records in the desk what is known about both machines: what the other announced
    /// (`peer`) and this one's, if missing or their system changed. The name and color already
    /// there are not overwritten: they are the user's. It is a single desk change (one version).
    ///
    /// It happens on receiving the other's announcement, which arrives after its desk
    /// ([`Agent::set_peer`]): this way the new version comes from the higher of the two and does not overwrite
    /// a real change with one of these.
    fn learn_machines(&self, peer: Machine, info: &MachineInfo) {
        let _editing = self.edit();
        let mut desk = self.desk();
        let me = self.machine();
        let local = local_info(&self.cfg, me, &desk);
        let mine = learn(&mut desk, me, &local);
        let theirs = learn(&mut desk, peer, info);
        // Colors are assigned when they meet and are not repeated (D17).
        let colors = desk.machines.distinct_colors();
        // When pairing, the usual scenes with the real names (D6).
        let scenes = (mine || theirs) && scenes::add_defaults(&mut desk);
        if mine || theirs || colors || scenes {
            self.commit_desk(desk);
        }
    }

    /// The agent generated the configuration on start (T-082): the UI says so.
    pub fn set_generated(&self, generated: bool) {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generated = generated;
    }

    /// The network's real port (it may not be the config's, if it was busy).
    pub fn set_port(&self, port: u16) {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .port = Some(port);
    }

    pub fn setup(&self) -> SetupInfo {
        self.setup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The shared desk that holds now.
    pub fn desk(&self) -> Config {
        self.desk.borrow().clone()
    }

    /// Priority, cap and pause of the data with the other machine, one per path and shared
    /// by all the connections of that path (T-101). The network sets it on each connection and
    /// brings it the desk's `[data]`. Whoever measures the input latency (T-093) records it
    /// with `conn.qos().record_input_latency(..)` or `link_qos().get(path)`, on both
    /// machines.
    pub fn link_qos(&self) -> &LinkQos {
        &self.link_qos
    }

    /// Notifies every time the desk changes.
    pub fn subscribe_desk(&self) -> watch::Receiver<Config> {
        self.desk.subscribe()
    }

    /// Changes the desk from this machine (the arrangement editor, T-096): it is validated, the version
    /// goes up, it goes to `tandem.toml`, to the UI and to the other machine.
    pub fn set_desk(&self, desk: Config) -> Result<(), String> {
        desk.validate().map_err(|e| e.to_string())?;
        let _editing = self.edit();
        if self.desk() == desk {
            return Ok(());
        }
        self.commit_desk(desk);
        Ok(())
    }

    /// A desk change requested by the UI (`UiCommand::EditDesk`): it is applied, the
    /// version goes up and it goes to `tandem.toml`, to the UI and to the other machine. Returns the desk that
    /// results, or why it was not applied.
    pub fn edit_desk(&self, patch: DeskPatch) -> Result<Config, String> {
        match &patch {
            DeskPatch::Shortcuts(s) => {
                crate::config::shortcut_keys(&s.lock).map_err(|e| e.to_string())?;
            }
            DeskPatch::AddScene(s) | DeskPatch::EditScene(s) => {
                crate::config::shortcut_keys(&s.shortcut).map_err(|e| e.to_string())?;
                self.check_scene_monitors(s)?;
            }
            DeskPatch::Scenes(scenes) => {
                for s in scenes {
                    crate::config::shortcut_keys(&s.shortcut).map_err(|e| e.to_string())?;
                    self.check_scene_monitors(s)?;
                }
            }
            _ => {}
        }
        let _editing = self.edit();
        let current = self.desk_sync();
        let next = current
            .edit(patch, self.machine(), now_millis())
            .map_err(|e| e.to_string())?;
        if let Some(next) = next {
            self.commit_local(next);
        }
        Ok(self.desk())
    }

    /// The module switches of this machine come from the desk (`Desk.modules` of
    /// its side, D4): they are copied every time the desk changes, wherever it comes from. What
    /// cannot be turned on or off here is ignored.
    fn sync_switches(&self) {
        let me = self.machine();
        let desk = self.desk();
        self.modules.send_if_modified(|s| {
            let mut changed = false;
            for id in tandem_proto::ModuleId::ALL {
                if self.registry.check_switch(id).is_err() {
                    continue;
                }
                let on = desk.modules.get(me, id.as_str()) != Some(false);
                if s.set(id, on) {
                    tracing::info!(module = ?id, enabled = on, "module changed");
                    changed = true;
                }
            }
            changed
        });
    }

    /// "Undo" (`UiCommand::UndoDesk`): goes back to version `to_rev` of the history with a
    /// new change, which wins like any other.
    pub fn undo_desk(&self, to_rev: u64) -> Result<Config, String> {
        let _editing = self.edit();
        let current = self.desk_sync();
        let next = {
            let clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock
                .history
                .undo(&current, to_rev, self.machine(), now_millis())
                .map_err(|e| e.to_string())?
        };
        if let Some(next) = next {
            tracing::info!(to = to_rev, rev = next.stamp.rev, "desk undone");
            self.commit_local(next);
        }
        Ok(self.desk())
    }

    /// The latest desk versions, from the newest to the oldest.
    pub fn desk_history(&self) -> Vec<HistoryEntry> {
        self.sync
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .history
            .entries()
    }

    /// Notifies every time the other machine changes the desk.
    pub fn subscribe_desk_changes(&self) -> broadcast::Receiver<DeskChanged> {
        self.desk_changes.subscribe()
    }

    /// The shared desk with its stamp, as it is sent to the other machine. Both come from one
    /// consistent snapshot: writers update them under the `sync` lock, held here while reading.
    pub fn desk_sync(&self) -> DeskVersion {
        let clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
        DeskVersion {
            stamp: clock.stamp,
            desk: self.desk.borrow().clone(),
        }
    }

    /// Records in the history the version that holds now.
    fn record_current(&self) {
        let current = self.desk_sync();
        self.sync
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .history
            .record(current);
    }

    fn config_path(&self) -> Option<PathBuf> {
        self.config_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The desk changed here: new version, to `tandem.toml`, to the UI and to the other machine.
    pub fn desk_changed(&self) {
        let _editing = self.edit();
        self.bump_desk();
    }

    /// [`Agent::desk_changed`] with the change already taken ([`Agent::edit`]): the desk that
    /// holds in memory becomes a new version of this machine.
    fn bump_desk(&self) {
        let next = self
            .desk_sync()
            .change(self.desk(), self.machine(), now_millis());
        self.commit_local(next);
    }

    /// `desk` as a new version of this machine. It is not published in memory before
    /// [`Agent::commit_local`]: this way what the UI, the other machine and the tests see is already
    /// in `tandem.toml` (before it was visible an instant before being saved). It is called with the change
    /// taken ([`Agent::edit`]).
    fn commit_desk(&self, desk: Config) {
        let next = self.desk_sync().change(desk, self.machine(), now_millis());
        self.commit_local(next);
    }

    /// A new version made here: it stays in memory, in the history, in `tandem.toml`, in
    /// the UI and on the other machine. It is called with the change taken ([`Agent::edit`]).
    fn commit_local(&self, next: DeskVersion) {
        {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.stamp = next.stamp;
            clock.history.record(next.clone());
            // The file goes first (the desk in memory is never ahead of it), still under the
            // lock so `desk_sync` cannot see the new stamp with the old desk.
            self.save_desk(&next);
            self.desk.send_if_modified(|d| {
                let changed = *d != next.desk;
                d.clone_from(&next.desk);
                changed
            });
        }
        self.orch.peer().send(ControlMsg::Desk(Box::new(next)));
        self.sync_switches();
        // This machine's name and color (or its modules) may have changed.
        self.refresh_capabilities();
        self.settle_scene();
    }

    /// The desk the other machine sent: if it wins (D5), it stays here, live and in
    /// `tandem.toml`, and the UI receives the notice with what changed. The input engine
    /// recomputes the geometry with it.
    pub fn receive_desk(&self, theirs: DeskVersion) {
        // An older peer still sends the Spanish ids (T-209): same mapping as when reading the file.
        let mut theirs = theirs;
        theirs.desk.migrate_legacy_ids();
        // A shortcut with keys this version does not know could not be saved.
        if let Err(e) = crate::config::desk_keys(&theirs.desk) {
            tracing::warn!("desk from the other machine ignored: {e}");
            return;
        }
        let editing = self.edit();
        let mine = self.desk_sync();
        let Some(next) = mine.receive(&theirs) else {
            return;
        };
        tracing::info!(
            rev = next.stamp.rev,
            origin = %next.stamp.by,
            screens = next.desk.screens.len(),
            "desk updated from the other machine"
        );
        {
            let mut clock = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            clock.stamp = next.stamp;
            clock.history.record(next.clone());
            // The file goes first (the desk in memory is never ahead of it), still under the
            // lock so `desk_sync` cannot see the new stamp with the old desk.
            self.save_desk(&next);
            self.desk.send_if_modified(|d| {
                let changed = *d != next.desk;
                d.clone_from(&next.desk);
                changed
            });
        }
        let summary = mine.desk.summary_of_changes(&next.desk);
        let by = next.stamp.by;
        let rev = next.stamp.rev;
        drop(editing);
        self.sync_switches();
        if let Some(summary) = summary
            && by != self.machine()
        {
            // Without a connected UI there is nobody to notify, and that is not an error.
            let _ = self.desk_changes.send(DeskChanged {
                by,
                rev,
                summary,
                undo_to: mine.stamp.rev,
            });
        }
        // This machine's name and color may come from the other.
        self.refresh_capabilities();
        self.settle_scene();
    }

    /// What this machine says about itself: its system and, from the desk, its name and color.
    pub fn local_info(&self) -> MachineInfo {
        local_info(&self.cfg, self.machine(), &self.desk.borrow())
    }

    /// The desk to the file, together with the current side: both always go in the same
    /// write. It is called with the change taken ([`Agent::edit`]).
    fn save_desk(&self, sync: &DeskVersion) {
        if let Some(path) = self.config_path()
            && let Err(e) = crate::config_edit::set_desk(&path, self.machine(), sync)
        {
            tracing::warn!("the desk was not saved to {}: {e}", path.display());
        }
    }

    /// Notifies every time the user asks for the macOS permissions.
    pub fn subscribe_permission_requests(&self) -> watch::Receiver<u64> {
        self.permission_requests.subscribe()
    }

    /// Notifies every time the UI asks to lock or release the cursor (T-129).
    pub fn subscribe_lock_requests(&self) -> watch::Receiver<(u64, bool)> {
        self.lock_requests.subscribe()
    }

    /// What each machine can do, for the UI.
    pub fn capabilities(&self) -> DeskCapabilities {
        self.caps.borrow().clone()
    }

    /// Notifies every time what either of the two can do changes.
    pub fn subscribe_capabilities(&self) -> watch::Receiver<DeskCapabilities> {
        self.caps.subscribe()
    }

    /// Rebuilds what this machine can do; if it changed, tells the UI right away and
    /// the other with the changes of [`capabilities::ANNOUNCE_DEBOUNCE`] together.
    pub fn refresh_capabilities(&self) {
        let here = self.local_capabilities();
        let changed = self.caps.send_if_modified(|c| {
            let changed = c.here != here;
            c.here = here;
            changed
        });
        if changed {
            let peer = self.orch.peer();
            let caps = self.caps.subscribe();
            // Each request carries the current link and the last of the window holds; if the link
            // changes without another change, the new one already received the announcement ([`Agent::set_peer`]).
            self.announce.schedule(move || {
                let here = caps.borrow().here.clone();
                peer.send(ControlMsg::Capabilities(Box::new(here)));
            });
        }
    }

    /// What the other machine announced.
    pub fn set_peer_capabilities(&self, peer: Capabilities) {
        if peer.machine != self.machine().other() {
            tracing::warn!(
                "capabilities announcement of machine {} over the other's link: ignored",
                peer.machine
            );
            return;
        }
        self.learn_machines(peer.machine, &peer.info);
        self.learn_wol(peer.power.mac.as_deref());
        self.caps.send_if_modified(|c| {
            let changed = c.peer.as_ref() != Some(&peer);
            c.peer = Some(peer);
            changed
        });
        self.sync_monitor_peer();
    }

    /// The other machine left the desk: this one also forgets its key (T-152).
    pub fn receive_unpaired(self: &Arc<Self>) {
        let control = self
            .pairing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(c) = control
            && c.peer_left().is_ok()
        {
            tracing::info!("the other machine left the desk: its key is forgotten");
        }
    }

    /// What the plan may ask of each machine ([`capabilities::abilities`]).
    pub fn abilities(&self) -> Abilities {
        let caps = self.caps.borrow();
        capabilities::abilities(&caps.here, caps.peer.as_ref())
    }

    /// "Lock here": it only takes effect if this machine is the server and its input
    /// engine runs; if not, it is rejected so the UI does not think it was locked.
    fn lock_here(&self, on: bool) -> Reply {
        if self.input_status().server != self.machine() {
            return Reply::Rejected("this machine is not the one that has the keyboard".into());
        }
        // The engine subscribes only while a server session is armed.
        if self.lock_requests.receiver_count() == 0 {
            return Reply::Rejected("the input engine is not active on this machine".into());
        }
        self.lock_requests.send_modify(|r| *r = (r.0 + 1, on));
        Reply::Done
    }

    /// The keyboard and mouse, for the UI.
    pub fn input_status(&self) -> InputStatus {
        self.input_status.borrow().clone()
    }

    /// Notifies every time the keyboard and mouse change.
    pub fn subscribe_input(&self) -> watch::Receiver<InputStatus> {
        self.input_status.subscribe()
    }

    pub fn set_input_status(&self, status: InputStatus) {
        let changed = self.input_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
        // The hub and the permissions are part of what this machine announces.
        if changed {
            self.refresh_capabilities();
        }
    }

    /// What the other agent sends for the keyboard and mouse.
    /// File drag between the machines (T-121).
    pub fn drag(&self) -> &crate::drag::DragHub {
        &self.drag
    }

    pub fn inbox(&self) -> &PeerInbox {
        &self.inbox
    }

    pub fn orchestrator(&self) -> &Orchestrator {
        &self.orch
    }

    /// Connection with the other agent for the steps that fall to it. The first thing that goes over
    /// it is its copy of the desk and then what this machine can do (T-084): the announcement
    /// may change the desk ([`Agent::learn_machines`]), and by then the other already
    /// adopted the newest version.
    pub fn set_peer(&self, peer: Arc<dyn PeerLink>) {
        self.orch.set_peer(peer.clone());
        peer.send(ControlMsg::Desk(Box::new(self.desk_sync())));
        peer.send(ControlMsg::Capabilities(Box::new(self.capabilities().here)));
        // What it read from its monitors, if it owns any: the previous link may have
        // lost the last report.
        let reports = self.monitors.reports_here();
        if !reports.is_empty() {
            peer.send(ControlMsg::Monitors(reports));
        }
        peer.send(ControlMsg::Update(UpdateMsg::Version(VERSION.to_owned())));
    }

    /// Transaction progress, for the UI.
    pub fn subscribe_txns(&self) -> broadcast::Receiver<TxnEvent> {
        self.orch.subscribe()
    }

    /// Power notices about the other machine (for example, that it was considered off), for the UI.
    pub fn subscribe_power(&self) -> broadcast::Receiver<PowerEvent> {
        self.power.subscribe()
    }

    /// The peer and the link to it, for the UI.
    pub fn peer_status(&self) -> PeerStatus {
        self.peer_status.borrow().clone()
    }

    /// Notifies every time the peer or its link changes.
    pub fn subscribe_peer(&self) -> watch::Receiver<PeerStatus> {
        self.peer_status.subscribe()
    }

    pub fn set_peer_status(&self, status: PeerStatus) {
        self.peer_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
    }

    /// Whoever handles the pairing requests (the network, on start).
    pub fn set_pairing(&self, control: Arc<dyn PairingControl>) {
        *self.pairing.lock().unwrap_or_else(|e| e.into_inner()) = Some(control);
    }

    pub fn pairing_status(&self) -> PairingStatus {
        self.pairing_status.borrow().clone()
    }

    /// Notifies with each pairing step.
    pub fn subscribe_pairing(&self) -> watch::Receiver<PairingStatus> {
        self.pairing_status.subscribe()
    }

    pub fn set_pairing_status(&self, status: PairingStatus) {
        self.pairing_status.send_if_modified(|s| {
            let changed = *s != status;
            *s = status;
            changed
        });
    }

    fn pair(&self, f: impl FnOnce(&dyn PairingControl) -> Result<(), String>) -> Reply {
        let control = self
            .pairing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match control {
            Some(c) => match f(c.as_ref()) {
                Ok(()) => Reply::Done,
                Err(why) => Reply::Rejected(why),
            },
            None => Reply::Rejected(NO_NETWORK.into()),
        }
    }

    pub(crate) fn emit_power(&self, event: PowerEvent) {
        // Without a connected UI there is nobody to notify, and that is not an error.
        let _ = self.power.send(event);
    }

    pub fn machine(&self) -> Machine {
        *self.me.borrow()
    }

    pub fn config(&self) -> &AgentConfig {
        &self.cfg
    }

    pub fn store_path(&self) -> &std::path::Path {
        self.store.path()
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    /// Notifies every time the state changes.
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    /// Publishes the new state and saves it.
    ///
    /// It is published even if saving fails: the change already happened and the UI has to see it;
    /// the error is returned to be logged.
    pub fn set_state(&self, mut new: State) -> Result<(), StoreError> {
        // Wherever it comes from (the other machine, a transaction), this one is powered on.
        new.on.set(self.machine(), true);
        scenes::settle(&mut new, &self.desk.borrow());
        let changed = self.state.send_if_modified(|s| {
            if *s == new {
                false
            } else {
                *s = new;
                true
            }
        });
        if !changed {
            return Ok(());
        }
        // If the other powered off (or came back), its monitors change owner.
        self.sync_monitor_peer();
        self.persist()
    }

    pub fn persist(&self) -> Result<(), StoreError> {
        // If another thread panicked while saving, the lock does not protect data: continue.
        let _guard = self.persisting.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.state.borrow().clone();
        self.store.save(&state)
    }

    /// Answers a UI request.
    ///
    /// Power on and off start a background transaction and are answered with
    /// [`Reply::Accepted`]; progress arrives as [`TxnEvent`]. It must be called inside
    /// a tokio runtime.
    pub fn handle(self: &Arc<Self>, cmd: UiCommand) -> Reply {
        match cmd {
            UiCommand::GetState => Reply::State(self.state()),
            UiCommand::GetDesk => Reply::Desk(self.desk()),
            UiCommand::GetSetup => Reply::Setup(self.setup()),
            UiCommand::GetCapabilities => Reply::Capabilities(Box::new(self.capabilities())),
            UiCommand::KeyProbe { on } => {
                self.key_probe(on);
                Reply::Done
            }
            UiCommand::HeardKey => {
                self.heard_key();
                Reply::Done
            }
            UiCommand::SetKeyboard(machine) => {
                self.set_keyboard(machine);
                Reply::Done
            }
            UiCommand::PingPeer => self.ping_peer(),
            UiCommand::EditDesk {
                patch:
                    DeskPatch::Module {
                        machine,
                        module,
                        on,
                    },
            } => match ModuleId::parse(module.trim()) {
                Some(id) => match self.switch_module(machine, id, on) {
                    Reply::Done => Reply::Desk(self.desk()),
                    other => other,
                },
                None => Reply::Rejected(format!("\"{module}\" is not a Tandem module")),
            },
            UiCommand::EditDesk { patch } => match self.edit_desk(patch) {
                Ok(desk) => Reply::Desk(desk),
                Err(why) => Reply::Rejected(why),
            },
            UiCommand::UndoDesk { to_rev } => match self.undo_desk(to_rev) {
                Ok(desk) => Reply::Desk(desk),
                Err(why) => Reply::Rejected(why),
            },
            UiCommand::GetDeskHistory => Reply::DeskHistory(self.desk_history()),
            UiCommand::GetPeer => Reply::Peer(self.peer_status()),
            UiCommand::GetPairing => Reply::Pairing(self.pairing_status()),
            UiCommand::StartPairing => self.pair(|p| p.start()),
            UiCommand::ConfirmPairing => self.pair(|p| p.confirm()),
            UiCommand::CancelPairing => self.pair(|p| p.cancel()),
            UiCommand::Unpair => self.pair(|p| p.unpair()),
            UiCommand::GetInput => Reply::Input(self.input_status()),
            UiCommand::RequestPermissions => {
                self.permission_requests.send_modify(|n| *n += 1);
                Reply::Done
            }
            UiCommand::LockHere { on } => self.lock_here(on),
            UiCommand::SetWol { mac } => self.set_wol_mac(mac.as_deref()),
            UiCommand::SetPower { machine, on } => self.start_power(machine, on),
            UiCommand::ApplyScene { id } => self.apply_scene(&id),
            UiCommand::SetModule {
                machine,
                module,
                on,
            } => self.switch_module(machine, module, on),
            UiCommand::GetMonitors => Reply::Monitors(self.monitors()),
            UiCommand::SwitchMonitor { monitor, input } => {
                self.start_switch_monitor(monitor, input)
            }
            UiCommand::Rescan { machine } => self.rescan_machine(machine),
            // The subscription belongs to the connection: the IPC handles it ([`Agent::watch_resources`]).
            UiCommand::WatchResources { .. } => {
                Reply::Rejected("resources are requested over an IPC connection".into())
            }
            UiCommand::GetUpdate => Reply::Update(self.update_status()),
            UiCommand::UpdatePeer { version } => self.update_peer(version),
            UiCommand::GetTunnels => Reply::Tunnels(self.tunnels.view()),
            // They wait for the other machine and only a client can ask for them: the IPC handles them
            // ([`crate::tunnels::handle`]).
            UiCommand::OpenTunnel { .. }
            | UiCommand::CloseTunnel { .. }
            | UiCommand::FilesPut { .. }
            | UiCommand::FilesGet { .. }
            | UiCommand::PortForward { .. }
            | UiCommand::StopForward { .. }
            | UiCommand::Exec { .. }
            | UiCommand::Kill { .. }
            | UiCommand::BringBack { .. } => Reply::Rejected(crate::tunnels::CLIENTS_ONLY.into()),
            // Also the connection's: the subscription and the measurement, which takes a few seconds
            // ([`Agent::measure_link`]).
            UiCommand::WatchNetwork { .. } | UiCommand::MeasureLink { .. } => {
                Reply::Rejected("the network is requested over an IPC connection".into())
            }
            // The subscription belongs to the connection ([`Agent::watch_cursor`]).
            UiCommand::WatchCursor { .. } => {
                Reply::Rejected("the cursor is requested over an IPC connection".into())
            }
            // Read only: what already reached this machine (D9).
            UiCommand::GetProcesses { tunnel } => match self.tunnels.processes(tunnel) {
                Some(list) => Reply::Processes(list),
                None => Reply::Rejected(format!("there is no tunnel {tunnel}")),
            },
            UiCommand::Output {
                tunnel,
                process,
                from,
                max,
            } => match self.tunnels.output(
                tunnel,
                process,
                from,
                usize::try_from(max).unwrap_or(usize::MAX),
            ) {
                Ok(out) => Reply::Output(Box::new(out)),
                Err(e) => Reply::Rejected(e.to_string()),
            },
        }
    }

    /// This machine's tunnels (the ones it hosts and the ones it opened).
    pub fn files(&self) -> &tandem_files::Files {
        &self.files
    }

    pub fn workspaces(&self) -> &tandem_workspace::Workspaces {
        &self.workspaces
    }

    pub fn tunnels(&self) -> &tandem_tunnel::Tunnels {
        &self.tunnels
    }

    /// Powering on is just waking `machine`; powering off is its clean shutdown. Asking for what already
    /// is is an empty transaction.
    fn start_power(self: &Arc<Self>, machine: Machine, on: bool) -> Reply {
        let name = self.machine_name(machine);
        let what = if on {
            format!("power on the {name}")
        } else {
            format!("power off the {name}")
        };
        let abilities = self.abilities();
        self.start_plan(&what, move |from| {
            if on {
                Plan::power_on(from, machine, &abilities)
            } else {
                Ok(Plan::power_off(from, machine))
            }
        })
    }

    fn start_plan(
        self: &Arc<Self>,
        what: &str,
        plan_for: impl FnOnce(&State) -> Result<Plan, PlanError>,
    ) -> Reply {
        self.start_txn(what, plan_for, None)
    }

    /// A background transaction; with `scene`, the one that applies that scene: it does its
    /// scene steps and, if everything goes well, it stays as the active one.
    fn start_txn(
        self: &Arc<Self>,
        what: &str,
        plan_for: impl FnOnce(&State) -> Result<Plan, PlanError>,
        scene: Option<scenes::SceneRun>,
    ) -> Reply {
        let Some(txn) = self.orch.try_begin() else {
            return Reply::Rejected(BUSY.into());
        };
        let busy = Busy(self.clone());
        let from = self.state();
        let plan = match plan_for(&from) {
            Ok(p) => p,
            Err(e) => return Reply::Rejected(e.to_string()),
        };
        tracing::info!(txn, what, steps = plan.steps.len(), "transaction");
        tokio::spawn(async move {
            let agent = busy.0.clone();
            let mut state = from;
            let outcome = match &scene {
                Some(run) => {
                    let steps = run.steps.clone();
                    agent.orch.run_scene(txn, &mut state, &plan, steps).await
                }
                None => agent.orch.run(txn, &mut state, &plan).await,
            };
            if let (Some(run), Ok(_)) = (&scene, &outcome) {
                state.scene = Some(run.id.clone());
            }
            match &outcome {
                Ok(_) => tracing::info!(txn, "transaction applied"),
                Err(e) => tracing::warn!(txn, "transaction failed: {e}"),
            }
            // First the state, then the notice: whoever receives it already sees the new state.
            if let Err(e) = agent.set_state(state.clone()) {
                tracing::warn!("{e}");
            }
            // Free before the notice: whoever receives it can already ask for the next change.
            drop(busy);
            agent.orch.finish(txn, &state, &outcome);
        });
        Reply::Accepted { txn }
    }
}

/// What this machine says about itself: the system is always the real one; the name and color are
/// the desk's if it already has them (the user can change them) or, if not, the system's name
/// and the color of its side.
///
/// If the winning desk does not carry it (the other wrote it before knowing it), the one from
/// `tandem.toml` at startup holds: the name the user chose is not lost.
fn local_info(cfg: &AgentConfig, me: Machine, desk: &Config) -> MachineInfo {
    if let Some(saved) = desk.machines.get(me) {
        return MachineInfo {
            os: cfg.os,
            name: saved.name.clone(),
            color: saved.color.clone(),
        };
    }
    let fresh = MachineInfo::new(me, cfg.os, capabilities::host_name());
    match cfg.desk.machines.get(cfg.machine) {
        Some(first) => MachineInfo {
            name: first.name.clone(),
            // A default color is the side's: with the new side, the new one.
            color: if tandem_core::DEFAULT_COLORS.contains(&first.color.as_str()) {
                fresh.color
            } else {
                first.color.clone()
            },
            os: cfg.os,
        },
        None => fresh,
    }
}

/// Sets `info` as what is known about `side` if nothing was known or the system changed. The
/// name and color that were already there are respected. `true` if the desk changed.
fn learn(desk: &mut Config, side: Machine, info: &MachineInfo) -> bool {
    match desk.machines.get(side) {
        Some(saved) if saved.os == info.os => false,
        Some(saved) => {
            let updated = MachineInfo {
                os: info.os,
                ..saved.clone()
            };
            desk.machines.set(side, Some(updated));
            true
        }
        None => {
            desk.machines.set(side, Some(info.clone()));
            true
        }
    }
}

/// Releases the orchestrator when the transaction ends, even if the task panics.
struct Busy(Arc<Agent>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.orch.end();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;
    use std::time::Duration;

    use tandem_core::{Scale, ScreenSpec, Side, Step};

    use super::*;
    use crate::config::fixtures;
    use crate::orchestrator::STEP_NOT_IMPLEMENTED;
    use crate::orchestrator::tests::{FakePeer, FakeSteps};

    pub fn agent(dir: &Path) -> Agent {
        Agent::new(
            fixtures::desk_with_wol(),
            StateStore::new(dir.join("state.json")),
        )
    }

    #[test]
    fn desk_sync_never_mixes_a_stamp_with_another_desk() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let done = Arc::new(AtomicBool::new(false));
        // Each version carries its own rev as the desk name, so a snapshot is consistent
        // only if stamp and name agree.
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (a, done) = (a.clone(), done.clone());
                std::thread::spawn(move || {
                    let mut seen = 0u32;
                    while !done.load(Ordering::Relaxed) {
                        let v = a.desk_sync();
                        if v.stamp.rev > 0 {
                            assert_eq!(
                                v.desk.name.as_deref(),
                                Some(v.stamp.rev.to_string().as_str()),
                                "stamp and desk come from different versions"
                            );
                        }
                        seen += 1;
                    }
                    seen
                })
            })
            .collect();
        for _ in 0..3000 {
            let _editing = a.edit();
            let current = a.desk_sync();
            let mut desk = current.desk;
            desk.name = Some((current.stamp.rev + 1).to_string());
            a.commit_desk(desk);
        }
        done.store(true, Ordering::Relaxed);
        for r in readers {
            assert!(r.join().unwrap() > 0);
        }
    }

    #[test]
    fn lock_here_is_rejected_without_input_engine() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        // Without an input engine there is nobody to listen to the request.
        assert!(matches!(
            a.handle(UiCommand::LockHere { on: true }),
            Reply::Rejected(_)
        ));
        assert_eq!(*a.lock_requests.borrow(), (0, false));
    }

    #[test]
    fn lock_here_is_rejected_when_not_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let _engine = a.subscribe_lock_requests();
        let mut st = a.input_status();
        st.server = a.machine().other();
        a.set_input_status(st);
        assert!(matches!(
            a.handle(UiCommand::LockHere { on: true }),
            Reply::Rejected(_)
        ));
    }

    #[test]
    fn lock_here_is_accepted_on_the_server_with_engine() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let engine = a.subscribe_lock_requests();
        let mut st = a.input_status();
        st.server = a.machine();
        a.set_input_status(st);
        assert_eq!(a.handle(UiCommand::LockHere { on: true }), Reply::Done);
        assert_eq!(*engine.borrow(), (1, true));
    }

    #[test]
    fn starts_from_initial_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert_eq!(a.machine(), Machine::A);
        assert_eq!(a.state(), State::initial(Machine::A));
    }

    #[test]
    fn get_state_replies_with_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(a.handle(UiCommand::GetState), Reply::State(a.state()));
    }

    #[test]
    fn starts_unpaired_and_reports_the_peer() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetPeer),
            Reply::Peer(PeerStatus::Unpaired)
        );
        let mut rx = a.subscribe_peer();
        let paired = PeerStatus::Paired {
            name: "PC".into(),
            link: None,
        };
        a.set_peer_status(paired.clone());
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), paired);
        // The same thing again is not a change.
        a.set_peer_status(paired.clone());
        assert!(!rx.has_changed().unwrap());
        assert_eq!(a.handle(UiCommand::GetPeer), Reply::Peer(paired));
    }

    #[test]
    fn pairing_without_network_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetPairing),
            Reply::Pairing(PairingStatus::Idle)
        );
        for cmd in [
            UiCommand::StartPairing,
            UiCommand::ConfirmPairing,
            UiCommand::CancelPairing,
            UiCommand::Unpair,
        ] {
            assert_eq!(a.handle(cmd), Reply::Rejected(NO_NETWORK.into()));
        }
    }

    /// Records what is asked of it; `confirm` fails so the reason reaches the UI.
    #[derive(Default)]
    struct FakePairing(Mutex<Vec<&'static str>>);

    impl PairingControl for FakePairing {
        fn start(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("start");
            Ok(())
        }
        fn confirm(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("confirm");
            Err("no code".into())
        }
        fn cancel(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("cancel");
            Ok(())
        }
        fn unpair(&self) -> Result<(), String> {
            self.0.lock().unwrap().push("unpair");
            Ok(())
        }
    }

    #[test]
    fn pairing_commands_go_to_the_network_and_status_is_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let fake = Arc::new(FakePairing::default());
        a.set_pairing(fake.clone());
        assert_eq!(a.handle(UiCommand::StartPairing), Reply::Done);
        assert_eq!(
            a.handle(UiCommand::ConfirmPairing),
            Reply::Rejected("no code".into())
        );
        assert_eq!(a.handle(UiCommand::CancelPairing), Reply::Done);
        assert_eq!(a.handle(UiCommand::Unpair), Reply::Done);
        assert_eq!(
            *fake.0.lock().unwrap(),
            ["start", "confirm", "cancel", "unpair"]
        );

        let mut rx = a.subscribe_pairing();
        a.set_pairing_status(PairingStatus::Searching);
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), PairingStatus::Searching);
        a.set_pairing_status(PairingStatus::Searching);
        assert!(!rx.has_changed().unwrap());
        assert_eq!(
            a.handle(UiCommand::GetPairing),
            Reply::Pairing(PairingStatus::Searching)
        );
    }

    #[test]
    fn get_desk_replies_with_the_configured_desk() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        assert_eq!(
            a.handle(UiCommand::GetDesk),
            Reply::Desk(a.config().desk.clone())
        );
    }

    fn steps_of(events: &[TxnEvent]) -> Vec<Step> {
        match events.first() {
            Some(TxnEvent::Started { steps, .. }) => steps.clone(),
            other => panic!("Started was expected: {other:?}"),
        }
    }

    /// Notices of a transaction up to its outcome.
    pub async fn until_outcome(rx: &mut broadcast::Receiver<TxnEvent>) -> Vec<TxnEvent> {
        let mut events = Vec::new();
        loop {
            let ev = rx.recv().await.unwrap();
            let end = matches!(ev, TxnEvent::Committed { .. } | TxnEvent::Failed { .. });
            events.push(ev);
            if end {
                return events;
            }
        }
    }

    fn with_fakes(dir: &Path, local: FakeSteps, peer: FakeSteps) -> Arc<Agent> {
        let a = Agent::with_local_steps(
            fixtures::desk_with_wol(),
            StateStore::new(dir.join("state.json")),
            Arc::new(local),
        );
        a.set_peer(Arc::new(FakePeer(Arc::new(peer))));
        Arc::new(a)
    }

    #[tokio::test]
    async fn power_on_only_wakes_the_pc() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPower {
                machine: Machine::B,
                on: true
            }),
            Reply::Accepted { .. }
        ));
        let events = until_outcome(&mut rx).await;
        assert_eq!(
            steps_of(&events),
            [Step::PowerOn {
                machine: Machine::B
            }]
        );
        let mut expected = State::initial(Machine::A);
        expected.on.b = true;
        assert_eq!(a.state(), expected);

        // Already on, asking again does nothing.
        a.handle(UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        });
        assert!(steps_of(&until_outcome(&mut rx).await).is_empty());
    }

    #[tokio::test]
    async fn power_off_is_only_the_clean_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(dir.path(), FakeSteps::default(), FakeSteps::default());
        let mut s = a.state();
        s.on.b = true;
        a.set_state(s).unwrap();
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPower {
                machine: Machine::B,
                on: false
            }),
            Reply::Accepted { .. }
        ));
        assert_eq!(
            steps_of(&until_outcome(&mut rx).await),
            [Step::PowerOff {
                machine: Machine::B
            }]
        );
        assert_eq!(a.state(), State::initial(Machine::A));
    }

    #[tokio::test]
    async fn without_backends_a_power_change_fails_and_keeps_the_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(agent(dir.path()));
        let mut rx = a.subscribe_txns();
        let Reply::Accepted { txn } = a.handle(UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        }) else {
            panic!("Accepted was expected");
        };
        let events = until_outcome(&mut rx).await;
        let initial = State::initial(Machine::A);
        assert!(events.iter().any(|e| matches!(
            e,
            TxnEvent::StepFailed { error, .. } if error.contains(STEP_NOT_IMPLEMENTED)
        )));
        assert_eq!(
            events.last(),
            Some(&TxnEvent::Failed {
                txn,
                state: initial.clone()
            })
        );
        assert_eq!(a.state(), initial);
    }

    #[tokio::test]
    async fn a_machine_that_cannot_be_woken_is_rejected_without_touching_state() {
        let dir = tempfile::tempdir().unwrap();
        // Without `[power.wol]` the Mac has nothing to wake the PC with.
        let a = Arc::new(Agent::new(
            fixtures::desk(),
            StateStore::new(dir.path().join("state.json")),
        ));
        let mut rx = a.subscribe_txns();
        assert!(matches!(
            a.handle(UiCommand::SetPower {
                machine: Machine::B,
                on: true
            }),
            Reply::Rejected(_)
        ));
        assert_eq!(a.state(), State::initial(Machine::A));
        assert!(rx.try_recv().is_err());
        // A rejection does not leave the orchestrator busy.
        assert!(a.orchestrator().try_begin().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn one_change_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let a = with_fakes(
            dir.path(),
            FakeSteps {
                delays: vec![(
                    Step::PowerOn {
                        machine: Machine::B,
                    },
                    Duration::from_secs(30),
                )],
                ..Default::default()
            },
            FakeSteps::default(),
        );
        let mut rx = a.subscribe_txns();
        let on = UiCommand::SetPower {
            machine: Machine::B,
            on: true,
        };
        assert!(matches!(a.handle(on.clone()), Reply::Accepted { .. }));
        assert_eq!(a.handle(on.clone()), Reply::Rejected(BUSY.into()));
        until_outcome(&mut rx).await;
        // Once the first one finishes, the next is accepted.
        assert!(matches!(a.handle(on), Reply::Accepted { .. }));
    }

    #[test]
    fn set_state_publishes_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let mut rx = a.subscribe();
        let mut s = a.state();
        s.on.b = true;
        a.set_state(s.clone()).unwrap();
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), s);
        // Another agent over the same directory starts with that state.
        assert_eq!(agent(dir.path()).state(), s);
    }

    // QA round 1: `persist` read the state and saved it without exclusion. With two
    // `set_state` at once, the one that read the old state could write last and the
    // file ended up with a state that was no longer the current one.
    #[test]
    fn concurrent_set_state_persists_the_final_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let base = a.state();
        for round in 0..20 {
            std::thread::scope(|scope| {
                for i in 0..8u32 {
                    let mut s = base.clone();
                    s.on.b = (i + round) % 2 == 0;
                    let a = &a;
                    scope.spawn(move || a.set_state(s).unwrap());
                }
            });
            let saved = StateStore::new(dir.path().join("state.json"))
                .load()
                .unwrap();
            assert_eq!(saved, Some(a.state()), "round {round}");
        }
    }

    #[test]
    fn same_state_does_not_notify() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let rx = a.subscribe();
        a.set_state(a.state()).unwrap();
        assert!(!rx.has_changed().unwrap());
    }

    fn screen(machine: Machine, id: &str, width: u32) -> ScreenSpec {
        ScreenSpec {
            machine,
            id: id.into(),
            x: 0,
            y: 0,
            width,
            height: 1440,
            scale: Scale::ONE,
        }
    }

    #[test]
    fn a_desk_from_the_other_machine_is_adopted_saved_and_announced() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, crate::autoconfig::generate(Machine::A)).unwrap();
        let a = agent(dir.path());
        a.set_config_file(file.clone());
        let rx = a.subscribe_desk();
        let mut changes = a.subscribe_desk_changes();
        let before = a.desk_sync().stamp.rev;
        let mut theirs = a.desk_sync();
        theirs.stamp = DeskStamp {
            rev: 5,
            at: 77,
            by: Machine::B,
        };
        theirs.desk.screens = vec![screen(Machine::B, "34", 3440)];
        theirs.desk.arrangement = Some(tandem_core::Arrangement {
            a_side: Side::Right,
            align: tandem_core::Align::Start,
        });
        a.receive_desk(theirs.clone());
        assert!(rx.has_changed().unwrap());
        assert_eq!(a.desk(), theirs.desk);
        assert_eq!(a.desk_sync(), theirs);
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.desk, theirs.desk);
        assert_eq!((saved.desk_rev, saved.desk_at), (5, 77));
        // The UI receives the notice with what changed and which version to go back to.
        assert_eq!(
            changes.try_recv().unwrap(),
            DeskChanged {
                by: Machine::B,
                rev: 5,
                summary: "Arrangement and screens".into(),
                undo_to: before,
            }
        );
        // An older one changes nothing.
        let mut old = theirs.clone();
        old.stamp.rev = 2;
        old.desk.screens.clear();
        a.receive_desk(old);
        assert_eq!(a.desk(), theirs.desk);
        assert!(changes.try_recv().is_err());
    }

    #[test]
    fn a_local_desk_change_bumps_the_version_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        let rev = a.desk_sync().stamp.rev;
        let mut desk = a.desk();
        desk.screens.push(screen(Machine::A, "laptop", 1512));
        a.set_desk(desk.clone()).unwrap();
        assert_eq!(a.desk(), desk);
        assert_eq!(a.desk_sync().stamp.rev, rev + 1);
        assert_eq!(a.desk_sync().stamp.by, Machine::A);
        assert!(a.desk_sync().stamp.at > 0, "with a date");
        // The same thing again does not bump the version; an invalid one is rejected.
        a.set_desk(desk.clone()).unwrap();
        assert_eq!(a.desk_sync().stamp.rev, rev + 1);
        desk.screens.push(screen(Machine::A, "laptop", 1512));
        assert!(a.set_desk(desk).is_err());
        assert_eq!(a.desk_sync().stamp.rev, rev + 1);
    }

    /// DESIGN §4.4: the UI edits with patches, can undo to any version of the
    /// history and see the history; what is invalid is rejected with the reason.
    #[tokio::test]
    async fn the_ui_edits_undoes_and_lists_the_desk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, crate::autoconfig::generate(Machine::A)).unwrap();
        let a = Arc::new(agent(dir.path()));
        a.set_config_file(file.clone());
        let start = a.desk_sync();
        let arr = tandem_core::Arrangement {
            a_side: Side::Up,
            align: tandem_core::Align::End,
        };
        let Reply::Desk(d) = a.handle(UiCommand::EditDesk {
            patch: DeskPatch::Arrangement(Some(arr)),
        }) else {
            panic!("no desk");
        };
        assert_eq!(d.arrangement, Some(arr));
        let lock = tandem_core::Shortcuts {
            lock: vec!["NotAKey".into()],
        };
        assert!(matches!(
            a.handle(UiCommand::EditDesk {
                patch: DeskPatch::Shortcuts(lock)
            }),
            Reply::Rejected(why) if why.contains("NotAKey")
        ));
        assert!(matches!(
            a.handle(UiCommand::EditDesk {
                patch: DeskPatch::Machine {
                    side: Machine::B,
                    name: Some("PC".into()),
                    color: None,
                }
            }),
            Reply::Rejected(_)
        ));
        let prefs = tandem_core::InputPrefs {
            drag_files: false,
            ..tandem_core::InputPrefs::default()
        };
        a.handle(UiCommand::EditDesk {
            patch: DeskPatch::Prefs(prefs),
        });
        assert_eq!(a.desk_sync().stamp.rev, start.stamp.rev + 2);
        let saved = AgentConfig::load(&file).unwrap();
        assert!(!saved.desk.prefs.drag_files);
        assert_eq!(saved.desk.arrangement, Some(arr));

        let Reply::DeskHistory(h) = a.handle(UiCommand::GetDeskHistory) else {
            panic!("no history");
        };
        let revs: Vec<u64> = h.iter().map(|e| e.stamp.rev).collect();
        assert_eq!(
            revs,
            [start.stamp.rev + 2, start.stamp.rev + 1, start.stamp.rev]
        );
        assert_eq!(h[0].summary, "Keyboard and mouse");

        // Undo to the beginning: exactly that desk, as a new version.
        let Reply::Desk(back) = a.handle(UiCommand::UndoDesk {
            to_rev: start.stamp.rev,
        }) else {
            panic!("it did not undo");
        };
        assert_eq!(back, start.desk);
        assert_eq!(a.desk_sync().stamp.rev, start.stamp.rev + 3);
        assert_eq!(AgentConfig::load(&file).unwrap().desk, start.desk);
        assert!(matches!(
            a.handle(UiCommand::UndoDesk { to_rev: 999 }),
            Reply::Rejected(_)
        ));
    }

    /// A `[[screens]]` written by hand is a change by this machine: the version goes up and it is
    /// saved with its fingerprint, so the other adopts it and does not overwrite it.
    #[test]
    fn a_hand_edited_desk_is_a_change_of_this_machine() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        let mut text = crate::autoconfig::generate(Machine::B).replacen(
            "\nmachine = \"b\"\n",
            "\nmachine = \"b\"\ndesk_rev = 4\ndesk_origin = \"a\"\n",
            1,
        );
        text.push_str(
            "\n[[screens]]\nmachine = \"b\"\nid = \"34\"\nx = 0\ny = 0\nwidth = 3440\nheight = 1440\n",
        );
        std::fs::write(&file, &text).unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        assert_eq!(a.desk_sync().stamp.rev, 5);
        assert_eq!(a.desk_sync().stamp.by, Machine::B);
        a.set_config_file(file.clone());
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.desk_rev, 5);
        assert!(saved.desk_sum.is_some());
        // Starting again without touching anything, it is no longer an edit.
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        assert_eq!(again.desk_sync().stamp.rev, 5);
        // A freshly generated file neither.
        let fresh = AgentConfig::from_toml(&crate::autoconfig::generate(Machine::B)).unwrap();
        let fresh = Agent::new(fresh, StateStore::new(dir.path().join("other.json")));
        assert_eq!(fresh.desk_sync().stamp.rev, 0);
    }

    /// T-209: a file with the Spanish ids (scenes, icon, screens, cables) is rewritten once with
    /// the English ones, without counting it as a change; a new one is not touched, and a second
    /// start leaves it as it is.
    #[test]
    fn a_desk_with_spanish_ids_is_rewritten_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        let old = "machine = \"a\"\ndesk_rev = 4\ndesk_origin = \"a\"\n\
                   [machines.a]\nos = \"macos\"\nname = \"MacBook\"\ncolor = \"#7aa7e6\"\n\
                   [machines.b]\nos = \"windows\"\nname = \"PC\"\ncolor = \"#a99be8\"\n\
                   [[screens]]\nmachine = \"a\"\nid = \"pantalla-1\"\nx = 0\ny = 0\nwidth = 1512\nheight = 982\n\
                   [[scenes]]\nid = \"trabajo\"\nname = \"Trabajo\"\nicon = \"puesto\"\n\
                   [[scenes]]\nid = \"solo-a\"\nname = \"Solo MacBook\"\nrequire_on = \"a\"\n\
                   [[cables]]\nmonitor = \"lc34-01\"\n[cables.a]\nscreen = \"pantalla-1\"\n";
        std::fs::write(&file, old).unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        assert!(cfg.legacy_desk);
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        assert_eq!(a.desk_sync().stamp.rev, 4, "a rename is not a change");
        a.set_config_file(file.clone());

        let text = std::fs::read_to_string(&file).unwrap();
        for gone in ["trabajo", "Trabajo", "puesto", "pantalla", "solo-a", "Solo"] {
            assert!(!text.contains(gone), "{gone}: {text}");
        }
        for kept in ["work", "only-a", "Only MacBook", "screen-1"] {
            assert!(text.contains(kept), "{kept}: {text}");
        }
        let saved = AgentConfig::load(&file).unwrap();
        assert!(!saved.legacy_desk);
        assert_eq!(saved.desk_rev, 4);
        // A second start changes neither the desk nor the file.
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        again.set_config_file(file.clone());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        assert_eq!(again.desk_sync().stamp.rev, 4);
    }

    /// Review of #152: the stored scene follows the desk read from the file. Renamed with it when
    /// the desk renamed it; kept when the desk kept `trabajo` because the user has a `work`.
    #[test]
    fn the_stored_scene_follows_the_desk_read_from_the_file() {
        let base = "machine = \"a\"\n[[scenes]]\nid = \"trabajo\"\nname = \"Trabajo\"\n";
        for (extra, want) in [
            ("", "work"),
            ("[[scenes]]\nid = \"work\"\nname = \"Mine\"\n", "trabajo"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let state = dir.path().join("state.json");
            std::fs::write(&state, r#"{"on":{"a":true,"b":false},"scene":"trabajo"}"#).unwrap();
            let cfg = AgentConfig::from_toml(&format!("{base}{extra}")).unwrap();
            let a = Agent::new(cfg, StateStore::new(&state));
            assert_eq!(a.state().scene.as_deref(), Some(want), "{extra}");
            let ids: Vec<_> = a.desk().scenes.iter().map(|s| s.id.clone()).collect();
            assert!(ids.iter().any(|id| id == want), "{ids:?}");
        }
    }

    /// T-209: a peer that still has the Spanish ids sends them; they are stored in English.
    #[tokio::test]
    async fn a_desk_from_an_older_peer_is_stored_in_english() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(&file, "machine = \"a\"\n").unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        a.set_config_file(file);
        let mut theirs = a.desk_sync();
        theirs.stamp = theirs.stamp.next(Machine::B, 1);
        theirs.desk.scenes = vec![tandem_core::Scene::new("trabajo", "Trabajo")];
        a.receive_desk(theirs);
        assert_eq!(a.desk().scenes[0].id, "work");
        assert_eq!(a.desk().scenes[0].name, "Work");
    }

    /// Review of #82: an already-paired T-099 desk (without the `scenes` key) receives the
    /// default scenes only once on loading, as a change by this machine; the one that clashes
    /// with the lock shortcut is left without a shortcut. If the user then deletes them all, they do not
    /// come back.
    #[test]
    fn a_paired_t099_desk_gets_the_default_scenes_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        let t099 = "machine = \"a\"\ndesk_rev = 7\ndesk_origin = \"b\"\n\
                    [machines.a]\nos = \"macos\"\nname = \"MacBook Pro\"\ncolor = \"#7aa7e6\"\n\
                    [machines.b]\nos = \"windows\"\nname = \"PC-Escritorio\"\ncolor = \"#a99be8\"\n\
                    [shortcuts]\nlock = [\"ControlLeft\", \"AltLeft\", \"Digit1\"]\n";
        std::fs::write(&file, t099).unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        let names: Vec<String> = a.desk().scenes.iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, ["Work", "Only MacBook Pro", "Only PC-Escritorio"]);
        assert!(a.desk().scenes[0].shortcut.is_empty(), "⌃⌥1 is the lock");
        assert_eq!(a.desk_sync().stamp.rev, 8, "a change by this machine");
        assert_eq!(a.desk_sync().stamp.by, Machine::A);
        a.set_config_file(file.clone());

        // Already saved: starting again is not another change.
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.desk.scenes, a.desk().scenes);
        assert_eq!(saved.desk_rev, 8);
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        assert_eq!(again.desk_sync().stamp.rev, 8);

        // All deleted on purpose: the file says so and they do not come back.
        again.set_config_file(file.clone());
        for id in ["work", "only-a", "only-b"] {
            again
                .edit_desk(DeskPatch::DeleteScene { id: id.into() })
                .unwrap();
        }
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("scenes = []"), "{text}");
        let empty = AgentConfig::load(&file).unwrap();
        assert!(empty.desk.scenes.is_empty());
        assert!(!empty.local_to_desk);
    }

    /// Review of #74: the pre-T-099 turned-off clipboard belonged to this machine and now
    /// is in the desk. It is a change by it: the version goes up (with its side and date) so the
    /// other adopts it, and it is saved that way.
    #[test]
    fn a_local_setting_moved_to_the_desk_bumps_the_version() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(
            &file,
            "machine = \"b\"\ndesk_rev = 3\ndesk_origin = \"a\"\n\n[input]\nclipboard = false\n",
        )
        .unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        assert!(cfg.local_to_desk);
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        let stamp = a.desk_sync().stamp;
        assert_eq!((stamp.rev, stamp.by), (4, Machine::B));
        assert!(stamp.at > 0);
        assert_eq!(a.desk().modules.get(Machine::B, "clipboard"), Some(false));
        a.set_config_file(file.clone());
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!((saved.desk_rev, saved.desk_origin), (4, Some(Machine::B)));
        assert!(!saved.local_to_desk && !saved.legacy_desk);
        // Starting again it is no longer a change.
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        assert_eq!(again.desk_sync().stamp.rev, 4);
    }

    /// A pre-T-099 `tandem.toml` (shortcut in `[input]`, offset in the arrangement)
    /// is rewritten in the new format without counting as anyone's change.
    #[test]
    fn a_desk_from_before_t099_is_rewritten_without_a_new_version() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(
            &file,
            "machine = \"a\"\ndesk_rev = 3\ndesk_origin = \"b\"\ndesk_sum = \"0123456789abcdef\"\n\n[input]\nlock_shortcut = [\"KeyL\"]\n\n[arrangement]\na_side = \"left\"\noffset = 0\n",
        )
        .unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        assert_eq!(a.desk_sync().stamp.rev, 3);
        a.set_config_file(file.clone());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("lock_shortcut") && !text.contains("offset"),
            "{text}"
        );
        let saved = AgentConfig::load(&file).unwrap();
        assert!(!saved.legacy_desk);
        assert_eq!((saved.desk_rev, saved.desk_origin), (3, Some(Machine::B)));
        assert_eq!(saved.desk.shortcuts.lock, ["KeyL"]);
        assert_eq!(saved.desk.arrangement, None);
        let again = Agent::new(saved, StateStore::new(dir.path().join("state.json")));
        assert_eq!(again.desk_sync().stamp.rev, 3);
    }

    fn caps_of(side: Machine, os: tandem_core::Os, name: &str) -> Capabilities {
        let cfg = fixtures::desk();
        capabilities::local(
            &cfg,
            side,
            MachineInfo::new(side, os, Some(name.into())),
            &InputStatus::new(Machine::A),
            &tandem_proto::ProcessInfo::default(),
            Vec::new(),
        )
    }

    /// What the other announces (and this one's) ends up in the desk, in a single version, and the
    /// name that was already there is not overwritten (DECISIONS A1, D17).
    #[test]
    fn the_announced_machines_are_kept_in_the_desk() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert!(
            a.desk().machines.is_empty(),
            "without another machine, nothing to save"
        );
        let rev = a.desk_sync().stamp.rev;
        a.set_peer_capabilities(caps_of(Machine::B, tandem_core::Os::MacOs, "Air"));
        let desk = a.desk();
        let theirs = desk.machines.get(Machine::B).unwrap();
        assert_eq!(
            (theirs.os, theirs.name.as_str()),
            (tandem_core::Os::MacOs, "Air")
        );
        assert_eq!(desk.machines.get(Machine::A), Some(&a.local_info()));
        assert_eq!(a.desk_sync().stamp.rev, rev + 1, "a single change");
        assert_eq!(a.machine_name(Machine::B), "Air");
        // Another announcement with another name does not overwrite it (the name is the user's); another system
        // does get recorded.
        a.set_peer_capabilities(caps_of(Machine::B, tandem_core::Os::MacOs, "Other"));
        assert_eq!(a.machine_name(Machine::B), "Air");
        assert_eq!(a.desk_sync().stamp.rev, rev + 1);
        a.set_peer_capabilities(caps_of(Machine::B, tandem_core::Os::Windows, "Other"));
        let theirs = a.machine_info(Machine::B).unwrap();
        assert_eq!(
            (theirs.os, theirs.name.as_str()),
            (tandem_core::Os::Windows, "Air")
        );
        // An announcement with this machine's side is not the other's: ignored.
        a.set_peer_capabilities(caps_of(Machine::A, tandem_core::Os::MacOs, "Yo"));
        assert_ne!(a.machine_name(Machine::A), "Yo");
    }

    /// The system this machine announces is its own, whichever side it is; the name and
    /// color are the desk's if it already has them.
    #[test]
    fn this_machine_announces_its_own_system_and_the_desk_name() {
        for os in tandem_core::Os::ALL {
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = fixtures::desk();
            cfg.machine = Machine::B;
            cfg.os = os;
            cfg.desk.machines.b = Some(MachineInfo {
                os: tandem_core::Os::MacOs,
                name: "Desktop".into(),
                color: "#5fbfb0".into(),
            });
            let a = Agent::new(cfg, StateStore::new(dir.path().join("s.json")));
            let here = a.capabilities().here;
            assert_eq!(here.machine, Machine::B);
            assert_eq!(here.info.os, os);
            assert_eq!(here.info.name, "Desktop");
            assert_eq!(here.info.color, "#5fbfb0");
        }
    }

    /// When pairing, this machine turns out to be the other side: everything that names a side is flipped,
    /// saved and keeps saying the same thing.
    #[tokio::test]
    async fn adopting_the_other_side_swaps_everything_that_names_a_side() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tandem.toml");
        std::fs::write(
            &file,
            "machine = \"a\"\n[input]\nkeyboard = \"a\"\n\n[[screens]]\nmachine = \"a\"\nid = \"laptop\"\nx = 0\ny = 0\nwidth = 1512\nheight = 982\n",
        )
        .unwrap();
        let cfg = AgentConfig::load(&file).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        a.set_config_file(file.clone());
        let side = a.subscribe_machine();
        let before = a.desk();
        a.adopt_side(Machine::A);
        assert!(
            !side.has_changed().unwrap(),
            "the same side changes nothing"
        );

        a.adopt_side(Machine::B);
        assert!(side.has_changed().unwrap());
        assert_eq!(a.machine(), Machine::B);
        // The default arrangement is not flipped (A stays to the left of B).
        let mut want = before.clone().swap_sides();
        want.arrangement = before.arrangement;
        assert_eq!(a.desk(), want);
        assert_eq!(a.keyboard_declared(), Some(Machine::B));
        assert!(a.state().is_on(Machine::B));
        assert_eq!(a.capabilities().here.machine, Machine::B);
        assert_eq!(a.input_status().server, Machine::B);
        let saved = AgentConfig::load(&file).unwrap();
        assert_eq!(saved.machine, Machine::B);
        assert_eq!(saved.input.keyboard, Some(Machine::B));
        assert_eq!(
            saved.desk.screens_of(Machine::B).next().unwrap().id,
            "laptop"
        );
        // Steps are distributed with the new side.
        let e = a
            .orchestrator()
            .execute_for_peer(&Step::PowerOff {
                machine: Machine::A,
            })
            .await
            .unwrap_err();
        assert!(e.contains("does not belong to"), "{e}");
    }

    /// Review of PR #68: switching sides while the desk and the other's announcement arrive
    /// wrote `tandem.toml` at once, and the file could end up with one side and the screens
    /// labeled from the other (on restart, the sides were flipped twice).
    #[test]
    fn changing_side_while_the_peer_talks_keeps_the_file_coherent() {
        for round in 0..30 {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("tandem.toml");
            std::fs::write(
                &file,
                "machine = \"a\"\n\n[[screens]]\nmachine = \"a\"\nid = \"laptop\"\nx = 0\ny = 0\nwidth = 1512\nheight = 982\n",
            )
            .unwrap();
            let a = Agent::new(
                AgentConfig::load(&file).unwrap(),
                StateStore::new(dir.path().join("state.json")),
            );
            a.set_config_file(file.clone());
            // What the other sends (which is A): its desk, with the screens of both.
            let mut theirs = a.desk_sync();
            theirs.stamp.rev = 9;
            theirs.stamp.by = Machine::A;
            theirs.desk.screens = vec![
                screen(Machine::A, "tower", 2560),
                screen(Machine::B, "laptop", 1512),
            ];
            let barrier = std::sync::Barrier::new(3);
            std::thread::scope(|s| {
                s.spawn(|| {
                    barrier.wait();
                    a.adopt_side(Machine::B);
                });
                s.spawn(|| {
                    barrier.wait();
                    a.receive_desk(theirs.clone());
                });
                s.spawn(|| {
                    barrier.wait();
                    a.set_peer_capabilities(caps_of(Machine::A, tandem_core::Os::Windows, "Tower"));
                });
            });
            let saved = AgentConfig::load(&file).unwrap();
            assert_eq!(saved.machine, a.machine(), "round {round}");
            assert_eq!(saved.desk, a.desk(), "round {round}");
            // Restarting with the keys' side there is no second side change.
            assert_eq!(saved.clone().with_side(Machine::B).desk, saved.desk);
        }
    }

    /// Review of PR #68: two fresh installations (default arrangement) do not end up with A on
    /// the right because one of the two switched sides.
    #[test]
    fn the_default_arrangement_is_not_flipped_when_changing_side() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(dir.path());
        assert_eq!(a.desk().arrangement, None);
        a.adopt_side(Machine::B);
        assert_eq!(a.desk().arrangement, None);
        // A declared and already synced one is physically kept.
        let dir = tempfile::tempdir().unwrap();
        let b = agent(dir.path());
        let mut desk = b.desk();
        desk.arrangement = Some(tandem_core::Arrangement {
            a_side: Side::Right,
            align: tandem_core::Align::Start,
        });
        b.set_desk(desk).unwrap();
        b.adopt_side(Machine::B);
        let arr = b.desk().arrangement.unwrap();
        assert_eq!(
            (arr.a_side, arr.align),
            (Side::Left, tandem_core::Align::Start)
        );
    }
}
