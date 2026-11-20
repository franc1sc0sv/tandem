// TypeScript mirror of the JSON of `tandem_ui::session` (serde). If it changes there, it changes here;
// the tests on both sides pin the shape.

import type { OutputWire, ProcessWire, ProjectWire } from "./tunnels";

/**
 * The two sides of the desk (T-097, DECISIONS A1): `a` is the one with the lower public key. Neither
 * side assumes a system: name, system and color come from its announcement (`MachineInfo`).
 */
export type Machine = "a" | "b";
export type Os = "macos" | "windows";

/** What a machine says about itself (`tandem_core::MachineInfo`). */
export interface MachineInfo {
  os: Os;
  name: string;
  /** Accent color in CSS (`#rrggbb`), chosen when pairing (D17). */
  color: string;
}
export type Side = "left" | "right" | "up" | "down";
export type Align = "start" | "center" | "end";

/** A screen of the declared arrangement (DESIGN §7), in the native coordinates of its machine. */
export interface ScreenSpec {
  machine: Machine;
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
}

/** A video input of a compatible monitor (`tandem_core::MonitorInput`). */
export type MonitorInput = "display_port" | "hdmi";

/** A scene of the desk (`tandem_core::Scene`): which machine shows each compatible monitor. */
export interface SceneJson {
  id: string;
  name: string;
  icon?: string;
  monitors: [string, Machine][];
  layout?: { a_side: Side; align: Align } | null;
  require_on?: Machine | null;
  /** Global shortcut, with `KeyboardEvent.code` names, all keys at once. */
  shortcut: string[];
}

/** The shared desk: the screen arrangement for keyboard and mouse. */
export interface Desk {
  /** What is known about each side (`null` if it has not announced itself yet). */
  machines: Record<Machine, MachineInfo | null>;
  screens: ScreenSpec[];
  /** Which side of `b` `a` is on and with what alignment (D18); `null` if undeclared: `a` to the
   * left of `b` and the whole edge crosses (T-099). */
  arrangement: { a_side: Side; align: Align } | null;
  /** Desk name (`desk_name`, T-159); without it the UI uses "My desk". */
  desk_name?: string | null;
  /** Each compatible monitor's cabling (`[[cables]]`, T-111). */
  cables?: CableJson[];
  scenes: SceneJson[];
  /** Shared keyboard and mouse (`[prefs]` of the desk). */
  prefs?: { cmd_ctrl: boolean; drag_files: boolean };
  /** Modules turned on per machine (`clipboard`…); what is missing counts as "on". */
  modules?: Record<Machine, Record<string, boolean>>;
  /** Desk shortcuts (`[shortcuts]`): the lock one, with `KeyboardEvent.code` names. */
  shortcuts?: { lock: string[] };
}

/** `tandem_proto::MonitorCompat`: serde default (external tag). `NoChannel`: the machine has its
 * EDID but no DDC channel (T-340). */
export type MonitorCompatJson = { Compatible: { protocol: number } } | { NotCompatible: { reason: string } } | "NoChannel";

/** One machine's end of a monitor cable (`tandem_core::CableEnd`). */
export interface CableEndJson {
  input?: MonitorInput | null;
  /** Which of that machine's screens the monitor is (declared, or learned from the EDID, T-340). */
  screen?: string | null;
}

/** How a compatible monitor is connected to both machines (`[[cables]]`, `tandem_core::MonitorCable`). */
export interface CableJson {
  monitor: string;
  a?: CableEndJson;
  b?: CableEndJson;
}

export type MonitorEventKindJson = "Boot" | "Input" | "SwitchRequested" | "SwitchDone" | "SwitchFailed" | "Power" | "Signal" | "Setting" | "Lost" | "Other";

export interface MonitorEventJson {
  /** Unix milliseconds, according to the owner's clock. */
  at_ms: number;
  kind: MonitorEventKindJson;
  input: MonitorInput | null;
}

/** A monitor as the agent sends it (`tandem_proto::MonitorView`). */
export interface MonitorJson {
  id: string;
  name: string;
  compat: MonitorCompatJson;
  seen_by: Machine[];
  via: [Machine, MonitorInput][];
  owner: Machine | null;
  report: { id: string; input: MonitorInput | null; power_on: boolean | null; signal: boolean | null; events: MonitorEventJson[] } | null;
}

export interface DeskState {
  /** Which machines are powered on. The agent's one always is for itself (T-084). */
  on: Record<Machine, boolean>;
  /** The active scene (D6), if it still holds. */
  scene?: string | null;
}

/** What a machine has and can do, according to its announcement (`tandem_ui::session::MachineView`). */
export interface MachineCaps {
  machine: Machine;
  name: string;
  os: Os;
  color: string;
  hub: boolean | null;
  input: {
    capture: boolean;
    inject: boolean;
    missing: ("accessibility" | "input_monitoring")[];
    /** The agent process: macOS grants permissions to that exact binary (T-090). */
    process: { exe: string; pid: number; started: number };
  };
  power: {
    wake_peer: boolean;
    wakeable: boolean;
    power_off: boolean;
    /** The MAC of its Ethernet card, so the other one stores it when pairing (T-159). */
    mac?: string | null;
    /** The MAC with which this machine wakes the other (`[power.wol]`); `null`: it cannot. */
    wol_mac?: string | null;
  };
  services: string[];
  /** Modules of its system (T-098); optional so an older agent can be read. */
  modules?: ModuleCaps[];
  /** Detected execution environments (T-105). */
  environments?: EnvCaps[];
  /** The hardware according to the scan (T-105). */
  hardware?: HardwareCaps;
}

/** The administrator service (T-131): what the app reports, not an invented spinner. */
export interface AdminView {
  /** `off` not installed, `pending` to install or half-done, `ok` active. */
  state: "off" | "pending" | "ok";
  /** There is an "Install" button; if not, `note` says why. */
  canInstall: boolean;
  note: string;
}

/** A module as its machine announces it (`tandem_ui::session::ModuleView`). */
export interface ModuleCaps {
  /** The id of the module: `input`, `clipboard`, `secure_desktop`… */
  id: string;
  name: string;
  detail: string;
  state: "active" | "off" | "unavailable";
  reason: string | null;
  switchable: boolean;
}

/** An execution environment (`tandem_ui::session::EnvView`). */
export interface EnvCaps {
  id: string;
  kind: "shell" | "wsl" | "docker";
  name: string;
  default: boolean;
  ready: boolean;
  detail: string;
  gpu: boolean;
}

export interface HardwareCaps {
  cpu: string | null;
  cores: number;
  /** Total RAM, in bytes. */
  memory: number;
  gpus: { name: string; vram: number | null }[];
}

/** A resource sample (`tandem_proto::Resources`): percentages in tenths, bytes. */
export interface ResourceSample {
  cpu: { load: number; cores: number[]; temperature: number | null };
  memory: { used: number; total: number };
  gpus: { name: string; load: number | null; vram_used: number | null; vram_total: number | null; temperature: number | null }[];
  disk: { free: number; total: number } | null;
}

/** This machine and the other one (`null` if it was never seen). */
export interface Caps {
  here: MachineCaps;
  peer: MachineCaps | null;
}

export type Link = "usb4" | "ethernet" | "wifi";

/** The other agent: without a peer, this machine works alone. `link` is `null` if it is not seen. */
export type PeerView = { kind: "unpaired" } | { kind: "paired"; name: string; link: Link | null };

/** How pairing is going (DESIGN §9): `code` is the 6 digits, with leading zeros. */
export type PairingView =
  | { kind: "idle" }
  | { kind: "searching" }
  | { kind: "comparing"; code: string; peer: string; confirmed: boolean }
  | { kind: "done"; peer: string }
  | { kind: "failed"; reason: string };

/** The shared keyboard and mouse (`tandem_ui::session::InputView`). */
export interface InputView {
  /** Where the keyboard and mouse are (the field is called that in the agent). */
  server: Machine;
  /** Whether the hub is on this machine; `null` without detection. */
  hub_here: boolean | null;
  /** The session with the other agent is established. */
  session: boolean;
  /** The cursor is locked on its machine (shortcut of the one that has the keyboard). */
  locked: boolean;
  /** macOS permissions that this machine is missing. */
  missing: ("accessibility" | "input_monitoring")[];
  /** Why the input engine of this machine does not work, if it does not. */
  problem: string | null;
  /** It is known where the keyboard is (hub, configuration or setup); if not, "undefined". */
  defined: boolean;
  /** The screens the engine reads here and those the other machine announced (T-150). */
  screens?: EngineScreen[];
  peer_screens?: EngineScreen[];
}

/** A screen as the input engine reads it (`tandem_proto::ScreenRect`). */
export interface EngineScreen {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Native units per logical point, in thousandths (1500 = 150 %). */
  scale_milli: number;
}

export type Snapshot =
  | { kind: "connecting" }
  | { kind: "connected"; agent: Machine; desk: Desk; state: DeskState; peer: PeerView; pairing: PairingView; input: InputView; caps: Caps; monitors: MonitorJson[] }
  | { kind: "unavailable"; endpoint: string | null; reason: string };

export type Step =
  | { step: "power_on"; machine: Machine }
  | { step: "power_off"; machine: Machine }
  | { step: "switch_monitor"; monitor: string; input: "display_port" | "hdmi" }
  /** Scene steps (T-100): which machine shows a monitor, the arrangement and the geometry. */
  | { step: "show_machine"; monitor: string; machine: Machine }
  | { step: "arrange"; layout: unknown }
  | { step: "geometry" };

export type TxnView =
  | { type: "started"; txn: number; steps: Step[] }
  | { type: "step_started"; txn: number; index: number }
  | { type: "step_done"; txn: number; index: number }
  | { type: "step_failed"; txn: number; index: number; error: string }
  /** Rollback: a step already done was undone because a later one failed. */
  | { type: "step_undone"; txn: number; index: number }
  | { type: "committed"; txn: number; state: DeskState }
  /** A step failed: `state` is what was left done. */
  | { type: "failed"; txn: number; state: DeskState };

/** Edge of a machine's desktop (`tandem_core::Side`). */
export type CrossEdge = "left" | "right" | "up" | "down";

/** Where the cursor is, in the native coordinates of its machine (`tandem_proto::CursorSample`). */
export interface CursorSample {
  machine: Machine;
  /** Id of the screen that contains it; empty if they are not known. */
  screen: string;
  x: number;
  y: number;
}

/** The progress of a file drag between the machines (`tandem_proto::DragStatus`, T-121). */
export type DragStatus =
  | { kind: "copying"; name: string; index: number; total: number }
  | { kind: "copied"; total: number }
  | { kind: "dropped" }
  | { kind: "cancelled" }
  | { kind: "failed"; reason: string };

export type Notice =
  | { kind: "drag"; status: DragStatus }
  | { kind: "txn"; event: TxnView }
  /** The live cursor (T-128), at 30 Hz at most; only arrives with `watch_cursor`. */
  | { kind: "cursor"; sample: CursorSample }
  /** The cursor crossed the `edge` edge from `from` to `to` (T-128). */
  | { kind: "crossed"; from: Machine; to: Machine; edge: CrossEdge }
  | { kind: "key_probe"; event: { armed: boolean; heard: Machine | null; second?: Machine | null } }
  | { kind: "power"; machine: Machine; reason: string }
  /** The other machine changed the desk and won (D5): "Changed from X · Undo". */
  | { kind: "desk_changed"; by: Machine; rev: number; summary: string; undo_to: number }
  /** The progress of a file copy between the machines. */
  | { kind: "transfer"; upload: boolean; path: string; done: number; total: number; finished: boolean };

export type Action =
  | { kind: "set_power"; machine: Machine; on: boolean }
  | { kind: "start_pairing" }
  | { kind: "confirm_pairing" }
  | { kind: "cancel_pairing" }
  | { kind: "unpair" }
  | { kind: "get_setup" }
  | { kind: "key_probe"; on: boolean }
  | { kind: "heard_key" }
  | { kind: "set_keyboard"; machine: Machine }
  | { kind: "ping_peer" }
  /** Receive (or stop receiving) the live cursor as `cursor` and `crossed` notices (T-128). */
  | { kind: "watch_cursor"; on: boolean }
  /** "Request permissions" (T-090): macOS shows its dialog in the name of `tandemd`. */
  | { kind: "request_permissions" }
  /** "Lock here" (T-129): locks (`on`) or releases the cursor on this machine. */
  | { kind: "lock_here"; on: boolean }
  /** Save the arrangement in the shared desk (`EditDesk { Arrangement }`, T-099). */
  | { kind: "set_arrangement"; a_side: Side; align: Align }
  /** The desk name (`EditDesk { Name }`); empty goes back to "My desk". */
  | { kind: "set_desk_name"; name: string }
  /** The MAC with which this machine wakes the other (T-159); `null` deletes it. */
  | { kind: "set_wol"; mac: string | null }
  | { kind: "set_module"; machine: Machine; module: string; on: boolean }
  | { kind: "set_prefs"; cmd_ctrl: boolean; drag_files: boolean }
  /** The desk lock shortcut (`EditDesk { Shortcuts }`); empty = no shortcut. */
  | { kind: "set_lock_shortcut"; lock: string[] }
  /** "Undo" of the change notice: returns the desk to version `to_rev` (D5). */
  | { kind: "undo_desk"; to_rev: number }
  | { kind: "rescan"; machine: Machine }
  /** Receive live resources while the visible screen shows them. */
  | { kind: "watch_resources"; on: boolean }
  /** Name or color of a machine (the shared desk). */
  | { kind: "set_machine"; machine: Machine; name?: string; color?: string }
  /** The Network screen asks for (or stops asking for) the network state, once per second (T-118). */
  | { kind: "watch_network"; on: boolean }
  /** Bandwidth test of a path (3 s); answers `bandwidth`. */
  | { kind: "measure_link"; path: Link }
  /** Apply a scene: a transaction with progress (T-116). */
  | { kind: "apply_scene"; id: string }
  /** Change the input of a compatible monitor, with the firmware verdict (T-110). */
  | { kind: "switch_monitor"; monitor: string; input: MonitorInput }
  | { kind: "add_scene"; scene: SceneJson }
  | { kind: "edit_scene"; scene: SceneJson }
  /** The processes of a tunnel, including those that ended recently (T-114); `tunnel` is the text id. */
  | { kind: "processes"; tunnel: string }
  /** The output of a process from the given cursors (T-114). */
  | { kind: "output"; tunnel: string; process: number; stdout: number; stderr: number }
  /** The tunnels open now (T-154): a window that opens or reloads does not wait for a change. */
  | { kind: "tunnels" };

/** Where the agent configuration came from (`tandem_proto::SetupInfo`). */
export interface AgentSetup {
  config_file: string | null;
  /** The agent generated it on its first start (T-082). */
  generated: boolean;
  port: number | null;
}

export type ActionResult =
  | { kind: "accepted"; txn: number }
  | { kind: "done" }
  | { kind: "rejected"; reason: string }
  | { kind: "unavailable"; reason: string }
  | { kind: "setup"; setup: AgentSetup }
  | { kind: "pong"; link: Link; millis: number }
  | { kind: "bandwidth"; link: Link; bytes_per_sec: number }
  | { kind: "processes"; processes: ProcessWire[] }
  | { kind: "output"; output: OutputWire }
  | { kind: "tunnels"; tunnels: ProjectWire[] };

// Global shortcuts (`tandem_ui::shortcuts`).
export type ShortcutAction = "toggle_window";

/** Action → canonical combination (`Ctrl+Alt+Digit0`), or `null` if it has none. */
export type Bindings = Record<ShortcutAction, string | null>;

export interface ShortcutsView {
  bindings: Bindings;
  /** Those the system would not let register, with the reason: the window handles them. */
  failed: Partial<Record<ShortcutAction, string>>;
  warning: string | null;
  file: string | null;
}

// First run on macOS (`tandem_ui::setup`).
export type Permission = "accessibility" | "input_monitoring" | "local_network";

export interface SetupView {
  /** There are permissions to grant on this system (macOS only). */
  available: boolean;
  /** Open setup on start: it has not been finished yet (on any system). */
  show: boolean;
  permissions: Permission[];
  /** How the login agent (`tandemd`) ended up, if the app tried to install it. */
  agent: { ok: boolean; text: string } | null;
}
