// What the UI shows of the desk, already digested for the screens. Filled in by the real agent
// (`live.ts`, over tandemd's IPC) or by demo mode (`demo.ts`). The screens only read from
// here: so each state can be drawn and tested without the agent.
//
// Modules and environments arrive over the IPC with T-098/T-105; meanwhile the real agent
// leaves those lists empty and demo mode simulates them.

import type { Arrangement, LayoutView } from "../arrangement";
import type { CrossSample } from "../crossing";
import type { AdminView, Bindings, BuiltinMode, CableStateJson, Machine, MonitorInput, Notice, Os, PairingView, PeerView, Permission, PowerStateJson, SceneJson, SetupView, ShortcutsView, TxnView, DragStatus } from "../model";
import type { PermissionState } from "../permissions";
import type { UpdateView } from "../updates";

export type { Machine, MonitorInput, Os };

export type ModuleState = "active" | "off" | "unavailable";

/** A module of a machine (D3): it exists where it compiles and says whether it is active and why not. */
export interface ModuleView {
  id: string;
  name: string;
  state: ModuleState;
  /** What it does on this machine, or why it is not available. */
  detail: string;
  /** The user can turn it on and off. */
  switchable: boolean;
}

/** A detected execution environment (T-105). */
export interface EnvView {
  id: string;
  kind: "shell" | "wsl" | "docker";
  name: string;
  /** The default of its kind. */
  default: boolean;
  ready: boolean;
  /** Tools, or why it is not ready. */
  detail: string;
}

/** The hardware according to the scan (D10). */
export interface HardwareView {
  cpu: string | null;
  /** Logical cores. */
  cores: number;
  /** Total RAM, in bytes. */
  memory: number;
  /** `vram` in bytes; `null` if it shares the RAM. */
  gpu: { name: string; vram: number | null } | null;
  /** The machine has no fan (the agent does not say yet). */
  fanless?: boolean;
}

/** Live resources for the Machine page (bytes and percentages 0–100). */
export interface LiveView {
  cpu: { percent: number; temp: number | null; history: number[] };
  ram: { used: number; total: number; history: number[] };
  gpu: {
    percent: number | null;
    vramUsed: number | null;
    vramTotal: number | null;
    temp: number | null;
    /** Fans, 0–100, if the system says so. */
    fans: number | null;
    history: number[];
  } | null;
  disk: { free: number; total: number } | null;
  /** Battery, if it has one (the agent does not send it yet). */
  battery: { percent: number; plugged: boolean } | null;
}

export interface Usage {
  /** 0–100. */
  percent: number;
  /** Short text to the right of the bar (`12.4/24 GB`, `18 %`). */
  label: string;
}

export interface Resources {
  cpu: Usage;
  ram: Usage;
  /** GPU or VRAM, if the machine has one. */
  gpu: Usage | null;
  gpuLabel: "GPU" | "VRAM";
  /** Latest CPU samples (0–100), from oldest to newest. */
  history: number[];
}

export type Power = "on" | "off" | "booting" | "lost";

export interface MachineView {
  id: Machine;
  name: string;
  os: Os;
  color: string;
  /** Line under the name: system and the most important of the hardware (comes from the scan, D10). */
  detail: string;
  /** It is the machine of this window. */
  here: boolean;
  power: Power;
  resources: Resources | null;
  modules: ModuleView[];
  /** System to display ("Windows 11", "macOS"). */
  system: string;
  hardware: HardwareView | null;
  environments: EnvView[];
  /** Live resources (Machine page); `null` without samples. */
  live: LiveView | null;
  /** Keyboards and mice it announces, if it lists them (setup, step 4). */
  inputDevices?: { name: string; via?: string }[];
  /** Only on the other machine: the MAC with which this one wakes it (T-159); `mac: null` = cannot. */
  wake?: { mac: string | null } | null;
  /** Only on a Mac: what it does with its built-in display while an external one is on (T-351). */
  builtinDisplay?: BuiltinMode;
}

export type LinkKind = "usb4" | "ethernet" | "wifi";

export interface LinkView {
  /** Path carrying the input; `null` if there is none. */
  kind: LinkKind | null;
  /** Measured bandwidth, in Gbps (`null` not measured). */
  gbps: number | null;
  /** Link latency, in ms. */
  ms: number | null;
  /** p99 of the input latency, in ms. */
  inputP99: number | null;
}

/** Role of a path (T-102): carries the traffic, waits as a backup, never responded, or went down. */
export type PathState = "primary" | "backup" | "unplugged" | "down";

/** One second of a path's history; `null` with no reply or without input. */
export interface LatencyPoint {
  rttMs: number | null;
  inputP99Ms: number | null;
}

/** A path of the Network screen (`V3-Network`). */
export interface PathView {
  kind: LinkKind;
  state: PathState;
  rttMs: number | null;
  inputP50Ms: number | null;
  inputP99Ms: number | null;
  /** The p99 tolerated for the input over this path (the target line). */
  targetMs: number;
  /** Last measured bandwidth, in Gbps. */
  gbps: number | null;
  /** Up to 60 samples, one per second, from oldest to newest. */
  history: LatencyPoint[];
  /** What is known about the link ("USB4 · 20 Gbps", "Wi-Fi 7 · home"); `null` if the agent does not say. */
  detail: string | null;
}

/** A path change ("switched to Wi-Fi in 0.6 s"). */
export interface PathSwitchView {
  /** Unix milliseconds. */
  at: number;
  from: LinkKind | null;
  to: LinkKind | null;
  /** How long it took if the previous one went down; `null` if it was a rise without a cut. */
  tookMs: number | null;
}

/** The network with the other machine (what the agent sends with `WatchNetwork`). */
export interface NetworkView {
  /** USB-C, Ethernet and Wi-Fi, in that order. */
  paths: PathView[];
  /** From newest to oldest (up to 50). */
  switches: PathSwitchView[];
  /** The path being measured. */
  measuring: LinkKind | null;
  /** Notices with the exact command the user runs by hand (T-094). */
  notices: { interface: string; message: string; command: string }[];
  /** When the last second of the history is (Unix milliseconds): dates the path changes. */
  now: number;
}

/** p99 tolerated for the input per path (DESIGN §7.1). */
export const INPUT_TARGET_MS: Record<LinkKind, number> = { usb4: 4, ethernet: 4, wifi: 20 };

const unplugged = (kind: LinkKind): PathView => ({
  kind,
  state: "unplugged",
  rttMs: null,
  inputP50Ms: null,
  inputP99Ms: null,
  targetMs: INPUT_TARGET_MS[kind],
  gbps: null,
  history: [],
  detail: null,
});

/** No network: the three paths with no cable. */
export const EMPTY_NETWORK: NetworkView = {
  paths: (["usb4", "ethernet", "wifi"] as LinkKind[]).map(unplugged),
  switches: [],
  measuring: null,
  notices: [],
  now: 0,
};

export type TunnelState = "busy" | "idle" | "closed";

/** A Claude tunnel (D8, read-only in the UI, D9). */
export interface TunnelView {
  /** The agent's id (16 hex): it travels as text because it does not fit in a JavaScript number. */
  id: string;
  project: string;
  /** The project folder at the origin, if the agent gave it. */
  projectPath: string | null;
  title: string;
  from: Machine;
  to: Machine;
  state: TunnelState;
  /** Processes running now. */
  processes: number;
  /** The git workspace branch (T-107); `null` while the agent does not report it. */
  branch: string | null;
  /** When it was opened, in Unix milliseconds. */
  created: number;
  /** Since when it has been idle (Unix ms), if known. */
  idleSince: number | null;
  /** When it closed (Unix ms); only on the closed ones. */
  closedAt: number | null;
  /** Where the processes run on the target machine ("WSL · Ubuntu"); `null` if the agent does not say. */
  env: string | null;
  /** Commands launched in the tunnel; `null` if the agent does not count them (the screen uses the ones it sees). */
  commands: number | null;
  /** Forwarded ports; `null` if the agent does not list them. */
  ports: number | null;
  /** Bytes that went (`up`) and came (`down`) through the tunnel. */
  traffic: { up: number; down: number };
}

/** A tunnel process (`GetProcesses`). */
export interface ProcessRow {
  id: number;
  command: string;
  pid: number | null;
  /** Ms Unix. */
  startedAt: number;
  state: "running" | "exited" | "killed";
  code: number | null;
}

/** How far the output of each stream was read. */
export interface OutputCursor {
  stdout: number;
  stderr: number;
}

/** What a process's output adds from a cursor. */
export interface OutputChunk {
  text: string;
  next: OutputCursor;
  /** The process ended and everything was already delivered. */
  done: boolean;
}

export interface SceneView {
  id: string;
  name: string;
  /** Global shortcut (`⌃⌥1`), or empty. */
  key: string;
  /** What it does, in one line. */
  desc: string;
  /** Which machine it shows (for the thumbnail); `null` if the scene changes no monitor. */
  shows: Machine | null;
  /** Which machine each compatible monitor shows (those the scene does not touch are absent). */
  monitors: { monitor: string; machine: Machine }[];
  /** Raw global shortcut (`KeyboardEvent.code`), to edit it. */
  shortcut: string[];
  /** The scene as the desk stores it, to edit it without losing what the screen does not show. */
  raw: SceneJson | null;
}

/** A monitor event, with its icon and its text (`V3-Monitors`). */
export interface MonitorEventView {
  /** Unix milliseconds. */
  at: number;
  kind: "switch" | "button" | "signal" | "boot" | "power" | "failed" | "other";
  text: string;
}

/** A monitor of the Monitors screen. D11: an incompatible one is only shown, with its reason. */
export interface MonitorUi {
  id: string;
  name: string;
  compat: { ok: true } | { ok: false; reason: string };
  /** Some machine sees it now. */
  connected: boolean;
  /** Who reads and controls it (D13). */
  owner: Machine | null;
  /** The input on screen (the real one); `null` if unknown. */
  input: MonitorInput | null;
  /** Which input each machine's cable goes to, if known. */
  ports: Partial<Record<Machine, MonitorInput>>;
  /** The machine whose signal is seen now, if deduced from `input` and the cables. */
  shows: Machine | null;
  /** From newest to oldest (up to 3). */
  events: MonitorEventView[];
  /** Which machine it shows, its cables and its power, as the agent derived them (T-349). */
  link?: LinkUi;
}

/** A monitor's link state for the UI (T-349). */
export interface LinkUi {
  kind: "shows" | "unplugged" | "standby" | "off" | "unknown";
  /** The machine shown (`shows`) or whose cable is unplugged (`unplugged`). */
  machine: Machine | null;
  power: PowerStateJson;
  cables: Record<Machine, CableStateJson>;
}

/** Pair with the other machine (what the agent sends). */
export interface PairState {
  peer: PeerView;
  pairing: PairingView;
}

/** A system permission of this machine (macOS, T-090). */
export interface PermissionRow {
  id: Permission;
  state: PermissionState;
}

export interface PermissionsView {
  /** This machine asks for permissions (macOS); on Windows the list stays empty. */
  needed: boolean;
  rows: PermissionRow[];
  /** "Grant the permission to this binary: …", if the agent said so. */
  binary: string | null;
}

/** The trouble states (`V3-States`). */
export type Trouble = "none" | "no-agent" | "peer-off" | "link-down" | "permissions";

/** Shared keyboard and mouse (Settings → Keyboard and mouse). */
export interface PrefsView {
  cmdCtrl: boolean;
  dragFiles: boolean;
  /** The shared clipboard is on in both machines. */
  clipboard: boolean;
}

/** A desk change made from the other machine (D5), for the "Undo" notice. */
export interface DeskChange {
  by: Machine;
  rev: number;
  summary: string;
  undoTo: number;
}

/** The progress of a file copy between the machines (notice with progress). */
export interface TransferView {
  upload: boolean;
  path: string;
  done: number;
  total: number;
  finished: boolean;
}

/** Where the configuration file and the logs of this machine are. */
export interface FilesView {
  config: string;
  logs: string;
}

export interface DeskView {
  prefs: PrefsView;
  /** The cursor is locked on this machine ("Lock here", T-157). */
  locked: boolean;
  /** The desk lock shortcut (`KeyboardEvent.code`); empty = no shortcut. */
  lockShortcut: string[];
  /** `connecting` while there is no first state. */
  ready: boolean;
  /** The desk name (next to the logo in `V3-Machine`); "My desk" if it has none. */
  deskName: string;
  trouble: Trouble;
  /** Why, in one line (for the trouble state). */
  troubleText: string;
  here: Machine;
  machines: Record<Machine, MachineView | null>;
  link: LinkView;
  /** Metrics per path (Network screen); they arrive only with the `network` topic. */
  network: NetworkView;
  tunnels: TunnelView[];
  scenes: SceneView[];
  activeScene: string | null;
  /** The monitors of both machines (D11: the incompatible ones, marked). */
  monitors: MonitorUi[];
  pair: PairState;
  permissions: PermissionsView;
  /** The screens and the arrangement (D18). */
  layout: LayoutView;
}

/** Event topics: each screen subscribes only to what it shows. */
export type Topic = "machines" | "resources" | "link" | "network" | "tunnels" | "scenes" | "monitors";

export const ALL_TOPICS: Topic[] = ["machines", "resources", "link", "network", "tunnels", "scenes", "monitors"];

/** What a source can or cannot do yet. */
export type Capability = "scenes" | "modules" | "power" | "measure" | "crossing" | "machine" | "monitors" | "forget" | "cursor" | "lock";

/** What arrives with the live cursor (T-128): its position or an edge crossing. */
export type CursorEvent = Extract<Notice, { kind: "cursor" | "crossed" }>;

export type PairActionKind = "start_pairing" | "confirm_pairing" | "cancel_pairing" | "unpair";

/**
 * Result of an action requested from the UI. `txn`: the transaction the agent opened for it; its
 * notices travel on another channel and can arrive before or after this reply.
 */
export type ActResult = { ok: true; text?: string; txn?: number } | { ok: false; reason: string };

/**
 * Where the data comes from. The store tells it which topics it wants (those of the visible screen)
 * and receives the new state every time it changes.
 */
export interface Source {
  start(onDesk: (d: DeskView) => void, onChange?: (c: DeskChange) => void, onTransfer?: (t: TransferView) => void): Promise<void>;
  /** Shared keyboard and mouse: the whole pair is sent. */
  setPrefs(p: { cmdCtrl: boolean; dragFiles: boolean }): Promise<ActResult>;
  /** What the Mac `machine` does with its built-in display (T-351). */
  setBuiltinDisplay(machine: Machine, mode: BuiltinMode): Promise<ActResult>;
  /** The desk lock shortcut (empty = no shortcut). */
  setLockShortcut(lock: string[]): Promise<ActResult>;
  /** "Undo" the other machine's change: returns the desk to version `rev`. */
  undoDesk(rev: number): Promise<ActResult>;
  /** Configuration file and logs (Settings → Advanced). */
  files(): Promise<FilesView>;
  /** Shows the configuration file or the logs folder in the file explorer. */
  openFiles(target: "config" | "logs"): Promise<ActResult>;
  /** Topics the visible screen needs live; the rest stop being sent. */
  watch(topics: Topic[]): void;
  applyScene(id: string): Promise<ActResult>;
  /** Change the input of a compatible monitor: a transaction with the firmware verdict. */
  switchMonitor(monitor: string, input: MonitorInput): Promise<ActResult>;
  /** Create (`isNew`) or save a scene in the shared desk: it reaches the other machine. */
  saveScene(scene: SceneJson, isNew: boolean): Promise<ActResult>;
  /** Forget an incompatible monitor that is no longer there. */
  forgetMonitor(id: string): Promise<ActResult>;
  /**
   * The progress of each transaction (scene, monitor, power) until the returned function
   * is called. `at` is the event time and `origin` the title of the `started`, if the source
   * fixes them (the demo, to capture).
   */
  watchTxn(onEvent: (e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void): Promise<() => void>;
  /** It is demo mode: the reference screens (States) show their full gallery. */
  readonly demo?: boolean;
  /** The progress of file drags between the machines; sources without an agent do not have it. */
  watchDrag?(onStatus: (s: DragStatus) => void): Promise<() => void>;
  /** The source time if it is not the clock's (the still demo, for deterministic screenshots). */
  now?(): number;
  setPower(machine: Machine, on: boolean): Promise<ActResult>;
  /** Measure the bandwidth of a path (by default, the one carrying the traffic). */
  measureLink(path?: LinkKind): Promise<ActResult>;
  setModule(machine: Machine, module: string, on: boolean): Promise<ActResult>;
  /** Rescan the environments of `machine` (the new ones arrive with the desk). */
  rescan(machine: Machine): Promise<ActResult>;
  /** Name or color of a machine (the shared desk, D4/D17). */
  editMachine(machine: Machine, edit: { name?: string; color?: string }): Promise<ActResult>;
  /** The desk name; empty goes back to the default (T-159). */
  setDeskName(name: string): Promise<ActResult>;
  /** The MAC with which the other machine is woken; `null` deletes it (T-159). */
  setWol(mac: string | null): Promise<ActResult>;
  /** Pair (`start_pairing`, `confirm_pairing`, `cancel_pairing`) or unpair. */
  pairAction(kind: PairActionKind): Promise<ActResult>;
  /** "Lock here" (T-129): the cursor does not cross to the other machine while it is locked. */
  lockHere(on: boolean): Promise<ActResult>;
  /** macOS shows its permissions dialog in the name of tandemd. */
  requestPermissions(): Promise<ActResult>;
  /** "Ask again" for Local Network (T-344): the agent tries the LAN at once, which brings up macOS' alert if it never asked. */
  checkLocalNetwork(): Promise<ActResult>;
  /** Opens the System Settings panel that grants the permission. */
  openPermission(p: Permission): Promise<ActResult>;
  /** Restarts tandemd so the permissions you just gave take effect. */
  restartAgent(): Promise<ActResult>;
  /** The real state of the administrator service. */
  adminStatus(): Promise<AdminView>;
  /** Installs it with privileges; rejects with the reason (canceled, not applicable, failed). */
  installAdmin(): Promise<AdminView>;
  /** The first-run permissions guide (macOS): whether it has to open by itself. */
  setup(): Promise<SetupView>;
  /** Marks the guide as seen. */
  finishSetup(): Promise<void>;
  shortcuts(): Promise<ShortcutsView>;
  /** Rejects with the reason if they cannot be used. */
  setShortcuts(b: Bindings): Promise<ShortcutsView>;
  /** The window shortcut handled by itself (if the system did not let it register). */
  hideMain(): Promise<void>;
  /** Settings → Updates (T-122). `onChange` receives each change (search, progress). */
  updates(onChange: (v: UpdateView) => void): Promise<UpdateView>;
  checkUpdates(): Promise<UpdateView>;
  /** Rejects with the reason if it cannot start. */
  updateAll(): Promise<UpdateView>;
  setAutoUpdate(on: boolean): Promise<UpdateView>;
  /** Saves the arrangement in the shared desk (T-099): it reaches the other machine. */
  setArrangement(a: Arrangement): Promise<ActResult>;
  /**
   * The crossing test: `onCross` receives each cursor crossing between the two machines until
   * the returned function is called. Rejects if the source does not report crossings.
   */
  watchCrossing(onCross: (s: CrossSample) => void): Promise<() => void>;
  /**
   * The live cursor (at 30 Hz at most) until the returned function is called.
   * Rejects if the agent does not offer it.
   */
  watchCursor(onEvent: (e: CursorEvent) => void): Promise<() => void>;
  /**
   * Listens for a key or a movement on each machine ("Keyboard and mouse" step): `onHeard`
   * receives the machine that heard it, until the returned function is called.
   */
  watchKeyProbe(onHeard: (m: Machine) => void): Promise<() => void>;
  /** Tells the agent that this window heard a key or a movement: it tells the other machine. */
  reportHeardKey?(): Promise<void>;
  /** Time to cross, if the source changes it (the demo, to capture the timeout). */
  readonly crossTimeoutMs?: number;
  /** The processes of a tunnel, including those that ended recently (T-104). */
  processes(tunnel: string): Promise<ProcessRow[]>;
  /** The output of a process from `from` (T-104). */
  output(tunnel: string, process: number, from: OutputCursor): Promise<OutputChunk>;
  /** What this source can do today (the real agent has no scenes or modules yet). */
  can(what: Capability): boolean;
  stop(): void;
}
