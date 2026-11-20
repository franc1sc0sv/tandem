// The real source: the state of tandemd over the IPC (through Tauri). Translates the
// agent's `Snapshot` into the screens' `DeskView`.
//
// Today the IPC sends a full snapshot with every change: `watch` only remembers which topics the
// visible screen asks for. Per-topic events (resources, tunnels, modules) arrive with
// T-098/T-102/T-104 and are subscribed to here.

import { ASSUMED_PPI, DEFAULT_ARRANGEMENT, EMPTY_LAYOUT, isVertical, ratioLabel, type Arrangement, type LayoutScreen, type LayoutView } from "../arrangement";
import type { Bridge } from "../bridge";
import type { CrossSample } from "../crossing";
import { nextLive, summaryOf } from "../machine";
import { machineColor, machineName, other } from "../machines";
import type { Action, ActionResult, AdminView, Bindings, Caps, Desk, DragStatus, Link, Machine, MachineCaps, MonitorInput, Side, Permission, ResourceSample, SceneJson, SetupView, ShortcutsView, Snapshot, TxnView } from "../model";
import { devicesOfHub } from "../wizard";
import { gbpsOf, measuredText, networkFromWire, primaryOf } from "../network";
import { hiddenScreens, monitorsUi } from "../monitors";
import { sceneView } from "../scenes";
import { grantTo, permissionState } from "../permissions";
import type { ActResult, Capability, CursorEvent, DeskChange, DeskView, FilesView, EnvView, HardwareView, LinkKind, LiveView, MachineView, ModuleView, NetworkView, OutputChunk, OutputCursor, PairActionKind, PermissionsView, PrefsView, ProcessRow, Source, Topic, TransferView, TunnelView } from "./types";
import { addTransfer, newMemory, outputChunk, processRow, tunnelsFromWire, type Memory, type ProjectWire } from "../tunnels";
import type { UpdateView } from "../updates";
import { EMPTY_NETWORK } from "./types";

/** The live resources of each side, built from the agent's samples. */
export type LiveByMachine = Partial<Record<Machine, LiveView | null>>;

/** How long to wait for the first cursor sample after a crossing before reporting it without a position. */
const CROSS_SETTLE_MS = 250;

const MAC_PERMISSIONS: Permission[] = ["accessibility", "input_monitoring", "local_network"];

const OS_NAME = { macos: "macOS", windows: "Windows" } as const;

/** The desk name when it has none of its own. */
export const DEFAULT_DESK_NAME = "My desk";

export const EMPTY_DESK: DeskView = {
  ready: false,
  lockShortcut: ["ScrollLock"],
  locked: false,
  deskName: DEFAULT_DESK_NAME,
  trouble: "none",
  troubleText: "",
  here: "a",
  machines: { a: null, b: null },
  link: { kind: null, gbps: null, ms: null, inputP99: null },
  network: EMPTY_NETWORK,
  tunnels: [],
  scenes: [],
  activeScene: null,
  monitors: [],
  pair: { peer: { kind: "unpaired" }, pairing: { kind: "idle" } },
  permissions: { needed: false, rows: [], binary: null },
  layout: EMPTY_LAYOUT,
  prefs: { cmdCtrl: true, dragFiles: true, clipboard: true },
};

/** The agent's snapshot does not have the shape this UI version expects. */
export class ShapeError extends Error {
  constructor(what: string) {
    super(`the agent replied with something this version of the app does not understand (${what})`);
    this.name = "ShapeError";
  }
}

const isSide = (m: unknown): m is Machine => m === "a" || m === "b";
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

/**
 * Checks what the UI reads from a connected snapshot. An agent of another version (for example
 * one with `mac`/`pc` sides) gives a trouble state with the reason, not a hung window.
 */
export function checkSnapshot(snap: unknown): asserts snap is Snapshot {
  if (!isObj(snap) || typeof snap.kind !== "string") throw new ShapeError("no “kind”");
  if (snap.kind !== "connected") return;
  if (!isSide(snap.agent)) throw new ShapeError(`side “${String(snap.agent)}”`);
  const desk = snap.desk;
  if (!isObj(desk) || !isObj(desk.machines) || !Array.isArray(desk.screens) || (desk.arrangement !== null && !isObj(desk.arrangement))) throw new ShapeError("desk");
  if (!isObj(snap.state) || !isObj(snap.state.on) || typeof snap.state.on.a !== "boolean" || typeof snap.state.on.b !== "boolean") throw new ShapeError("on");
  if (!isObj(snap.peer) || !isObj(snap.pairing)) throw new ShapeError("pairing");
  if (!Array.isArray(desk.scenes)) throw new ShapeError("scenes");
  if (!Array.isArray(snap.monitors)) throw new ShapeError("monitors");
  if (!isObj(snap.input) || !Array.isArray(snap.input.missing)) throw new ShapeError("input");
  const caps = snap.caps;
  if (!isObj(caps) || !isObj(caps.here) || !isSide(caps.here.machine) || !isObj(caps.here.input)) throw new ShapeError("capabilities");
  if (caps.peer !== null && (!isObj(caps.peer) || !isSide(caps.peer.machine))) throw new ShapeError("the other one’s capabilities");
}

/** Keyboard and mouse according to the desk; what the agent does not say counts as "on" (as in its `Default`). */
export function prefsOf(desk: Desk): PrefsView {
  const on = (m: Machine) => desk.modules?.[m]?.clipboard !== false;
  return { cmdCtrl: desk.prefs?.cmd_ctrl ?? true, dragFiles: desk.prefs?.drag_files ?? true, clipboard: on("a") && on("b") };
}

function permissionsOf(here: MachineCaps): PermissionsView {
  if (here.os !== "macos") return { needed: false, rows: [], binary: null };
  return { needed: true, rows: MAC_PERMISSIONS.map((id) => ({ id, state: permissionState(here, id) })), binary: grantTo(here) };
}

function modulesOf(c: MachineCaps | null): ModuleView[] {
  // If it is not available, the line says why (T-098: actionable reasons).
  return (c?.modules ?? []).map((m) => ({ id: m.id, name: m.name, state: m.state, detail: m.state === "unavailable" && m.reason ? m.reason : m.detail, switchable: m.switchable }));
}

function hardwareOf(c: MachineCaps | null): HardwareView | null {
  const h = c?.hardware;
  if (!h) return null;
  const g = h.gpus[0];
  return { cpu: h.cpu, cores: h.cores, memory: h.memory, gpu: g ? { name: g.name, vram: g.vram } : null };
}

function environmentsOf(c: MachineCaps | null): EnvView[] {
  return (c?.environments ?? []).map((e) => ({ id: e.id, kind: e.kind, name: e.name, default: e.default, ready: e.ready, detail: e.detail }));
}

function machineView(who: Machine, here: Machine, caps: Caps, snap: Extract<Snapshot, { kind: "connected" }>, live: LiveView | null): MachineView | null {
  const c: MachineCaps | null = caps.here.machine === who ? caps.here : caps.peer?.machine === who ? caps.peer : null;
  const info = snap.desk.machines?.[who] ?? null;
  if (!c && !info) return null;
  const os = c?.os ?? info?.os ?? "windows";
  const on = snap.state.on[who];
  const power = who === here || on ? "on" : "off";
  const shown = power === "on" ? live : null;
  return {
    id: who,
    name: machineName(who, caps, snap.desk),
    os,
    color: machineColor(who, caps, snap.desk),
    detail: OS_NAME[os] ?? os,
    here: who === here,
    power,
    resources: shown ? summaryOf(shown) : null,
    modules: modulesOf(c),
    system: OS_NAME[os] ?? os,
    hardware: hardwareOf(c),
    environments: environmentsOf(c),
    inputDevices: devicesOfHub(c?.hub),
    live: shown,
    wake: who === here ? null : { mac: caps.here.power.wol_mac ?? null },
  };
}

const LINK_KIND: Record<Link, DeskView["link"]["kind"]> = { usb4: "usb4", ethernet: "ethernet", wifi: "wifi" };

/** The summary of the path carrying the input; the numbers come from the network if it is being watched. */
function linkOf(kind: DeskView["link"]["kind"], network: NetworkView): DeskView["link"] {
  const p = kind && primaryOf(network)?.kind === kind ? primaryOf(network) : null;
  return { kind, gbps: p?.gbps ?? null, ms: p?.rttMs ?? null, inputP99: p?.inputP99Ms ?? null };
}

/** The desk the UI draws from the agent's snapshot. Pure; throws `ShapeError`. */
export function deskFromSnapshot(raw: Snapshot, live: LiveByMachine = {}, network: NetworkView = EMPTY_NETWORK, tunnels: TunnelView[] = []): DeskView {
  const snap: unknown = raw;
  checkSnapshot(snap);
  if (snap.kind === "connecting") return EMPTY_DESK;
  if (snap.kind === "unavailable") {
    return { ...EMPTY_DESK, ready: true, trouble: "no-agent", troubleText: snap.reason };
  }
  const here = snap.agent;
  const peer = other(here);
  const machines = { a: machineView("a", here, snap.caps, snap, live.a ?? null), b: machineView("b", here, snap.caps, snap, live.b ?? null) };
  const peerName = machineName(peer, snap.caps, snap.desk);
  const linked = snap.peer.kind === "paired" && snap.peer.link !== null;
  let trouble: DeskView["trouble"] = "none";
  let troubleText = "";
  if (snap.input.missing.length > 0) {
    trouble = "permissions";
    troubleText = "Permissions are missing on this machine";
  } else if (snap.peer.kind === "paired" && !snap.state.on[peer]) {
    trouble = "peer-off";
    troubleText = `${peerName} is off`;
  } else if (snap.peer.kind === "paired" && !linked) {
    trouble = "link-down";
    troubleText = `No link with ${peerName}`;
  }
  const monitors = monitorsUi(snap.monitors);
  // Shortcuts are written according to this machine's system, not the browser's.
  const mac = snap.caps.here.os === "macos";
  const name = (m: Machine) => machineName(m, snap.caps, snap.desk);
  return {
    ...EMPTY_DESK,
    ready: true,
    deskName: snap.desk.desk_name?.trim() || DEFAULT_DESK_NAME,
    monitors,
    scenes: snap.desk.scenes.map((sc) => sceneView(sc, { monitors, name, mac })),
    activeScene: snap.state.scene ?? null,
    prefs: prefsOf(snap.desk),
    lockShortcut: snap.desk.shortcuts?.lock ?? ["ScrollLock"],
    locked: snap.input.locked === true,
    trouble,
    troubleText,
    here,
    machines,
    link: linkOf(snap.peer.kind === "paired" && snap.peer.link ? LINK_KIND[snap.peer.link] : null, network),
    network,
    tunnels,
    pair: { peer: snap.peer, pairing: snap.pairing },
    permissions: permissionsOf(snap.caps.here),
    layout: layoutFromDesk(snap),
  };
}

/**
 * The arrangement screens according to the desk: those declared by each machine, or one per
 * machine if it declared none. The agent does not report the physical size yet: it is estimated
 * with [`ASSUMED_PPI`] from the logical points.
 */
export function layoutFromDesk(snap: Extract<Snapshot, { kind: "connected" }>): LayoutView {
  const screens: LayoutScreen[] = [];
  // A monitor that shows the other machine is not a screen of this one (T-340): drawn once.
  const hidden = hiddenScreens(snap.desk.cables, monitorsUi(snap.monitors));
  for (const m of ["a", "b"] as Machine[]) {
    const name = machineName(m, snap.caps, snap.desk);
    const known = snap.desk.machines?.[m] ?? (snap.caps.here.machine === m ? snap.caps.here : snap.caps.peer?.machine === m ? snap.caps.peer : null);
    if (!known) continue;
    const mac = known.os === "macos";
    const declared = snap.desk.screens.filter((s) => s.machine === m);
    // Without `[[screens]]` (the normal case after pairing) the ones the engine of each system reads are used.
    const engine = (snap.agent === m ? snap.input.screens : snap.input.peer_screens) ?? [];
    const all = declared.length > 0 ? declared : engine.map((s) => ({ id: s.id, x: s.x, y: s.y, width: s.width, height: s.height, scale: s.scale_milli / 1000 }));
    const own = all.filter((s) => !hidden[m].includes(s.id));
    // Every screen it has shows the other machine: nothing of it to draw.
    if (all.length > 0 && own.length === 0) continue;
    if (own.length === 0) {
      screens.push({ id: `desk-${m}`, machine: m, name, short: name.split(" ")[0] ?? name, ask: name, x: 0, y: 0, w: 16, h: 10, size: "—", res: "—", ratio: "—", scale: "—", usable: true, line: name });
      continue;
    }
    // The positions (`x`, `y`) are in the machine's coordinates: they are converted to points with the
    // scale of its first screen. It is approximate if the scales of its monitors differ.
    const base = mac ? 1 : own[0]?.scale || 1;
    for (const s of own) {
      // macOS gives points; Windows, physical pixels that the scale converts to logical ones.
      const logical = mac ? 1 : s.scale || 1;
      screens.push({
        id: `${m}:${s.id}`,
        machine: m,
        name: s.id,
        short: s.id,
        ask: s.id,
        x: s.x / base / ASSUMED_PPI,
        y: s.y / base / ASSUMED_PPI,
        w: s.width / logical / ASSUMED_PPI,
        h: s.height / logical / ASSUMED_PPI,
        size: "—",
        res: `${s.width} × ${s.height}`,
        ratio: ratioLabel(s.width, s.height),
        scale: `${Math.round(s.scale * 100)} %`,
        usable: true,
        line: name,
        native: { id: s.id, x: s.x, y: s.y, w: s.width, h: s.height },
      });
    }
  }
  const arr = snap.desk.arrangement;
  const arrangement: Arrangement = arr ? { aSide: arr.a_side, align: arr.align } : DEFAULT_ARRANGEMENT;
  return { screens, arrangement };
}

/** The trouble state of an error when talking to the agent. */
export function deskFromError(e: unknown): DeskView {
  return { ...EMPTY_DESK, ready: true, trouble: "no-agent", troubleText: e instanceof Error ? e.message : String(e) };
}

function result(r: ActionResult): ActResult {
  if (r.kind === "rejected" || r.kind === "unavailable") return { ok: false, reason: r.reason };
  return r.kind === "accepted" ? { ok: true, txn: r.txn } : { ok: true };
}

export class LiveSource implements Source {
  private topics: Topic[] = [];
  private last: Snapshot = { kind: "connecting" };
  private live: LiveByMachine = {};
  /** Live resources were requested from the agent. */
  private watching = false;
  private network: NetworkView = EMPTY_NETWORK;
  /** The live network was requested from the agent. */
  private watchingNetwork = false;
  /** The tunnels as the agent last sent them, and what the window remembers about them. */
  private groups: ProjectWire[] = [];
  private readonly memory: Memory = newMemory();
  private tunnels: TunnelView[] = [];
  /** Tunnels have arrived (by event or by request): a late request does not overwrite the newer one. */
  private gotTunnels = false;
  private askedTunnels = false;
  /** Who receives the live cursor; the agent only sends it while someone is listening. */
  private readonly cursorListeners = new Set<(e: CursorEvent) => void>();

  constructor(private readonly bridge: Bridge) {}

  async start(onDesk: (d: DeskView) => void, onChange?: (c: DeskChange) => void, onTransfer?: (t: TransferView) => void): Promise<void> {
    // An error reading a snapshot does not hang the window: it is shown as "no agent" with the reason.
    const push = (s: Snapshot) => {
      try {
        const d = deskFromSnapshot(s, this.live, s.kind === "connected" ? this.network : EMPTY_NETWORK, s.kind === "connected" ? this.tunnels : []);
        this.last = s;
        onDesk(d);
        if (s.kind === "connected") askTunnels();
      } catch (e) {
        this.last = { kind: "connecting" };
        onDesk(deskFromError(e));
      }
    };
    // The session only pushes the tunnels on connect and on each change: a window that opens
    // or reloads later requests them once (T-154). Any other response is ignored.
    const retunnel = () => {
      this.tunnels = tunnelsFromWire(this.groups, Date.now(), this.memory);
      if (this.last.kind === "connected") push(this.last);
    };
    const askTunnels = () => {
      if (this.askedTunnels || typeof this.bridge.act !== "function") return;
      this.askedTunnels = true;
      Promise.resolve(this.bridge.act({ kind: "tunnels" }))
        .then((r) => {
          if (r?.kind !== "tunnels" || this.gotTunnels) return;
          this.gotTunnels = true;
          this.groups = r.tunnels;
          retunnel();
        })
        .catch(() => {
          this.askedTunnels = false;
        });
    };
    await this.bridge.onSnapshot(push);
    // The "Changed from X · Undo" notice (D5): it only travels through the notices, not through the snapshot.
    void Promise.resolve(
      this.bridge.onNotice?.((n) => {
        if (n.kind === "desk_changed") onChange?.({ by: n.by, rev: n.rev, summary: n.summary, undoTo: n.undo_to });
        else if (n.kind === "transfer") onTransfer?.({ upload: n.upload, path: n.path, done: n.done, total: n.total, finished: n.finished });
      }),
    ).catch(() => {});
    // The samples arrive separately (one per second): each advances its machine's line.
    // The subscription is not awaited: the snapshot must not depend on that channel responding.
    void Promise.resolve(
      this.bridge.onResources?.((machine: Machine, r: ResourceSample) => {
        this.live = { ...this.live, [machine]: nextLive(this.live[machine] ?? null, r) };
        if (this.last.kind === "connected") push(this.last);
      }),
    ).catch(() => {});
    // The same with the network: each state is painted with the latest snapshot.
    void Promise.resolve(
      this.bridge.onNetwork?.((w) => {
        this.network = networkFromWire(w, Date.now());
        if (this.last.kind === "connected") push(this.last);
      }),
    ).catch(() => {});
    // The tunnels arrive on connect and on each change, without asking for them; the progress of the copies
    // feeds each one's traffic.
    void Promise.resolve(
      this.bridge.onTunnels?.((g) => {
        this.gotTunnels = true;
        this.groups = g;
        retunnel();
      }),
    ).catch(() => {});
    void Promise.resolve(
      this.bridge.onTransfer?.((t) => {
        addTransfer(this.memory, t);
        retunnel();
      }),
    ).catch(() => {});
    this.bindNotices().catch(() => {});
    push(await this.bridge.snapshot());
  }

  /** Resources and the network are requested only while a visible screen shows them. */
  watch(topics: Topic[]): void {
    this.topics = [...topics];
    if (typeof this.bridge.act !== "function") return;
    const resources = topics.includes("resources");
    if (resources !== this.watching) {
      this.watching = resources;
      // The session remembers the request and repeats it on reconnect; if it fails now, there is no agent.
      this.bridge.act({ kind: "watch_resources", on: resources }).catch(() => {});
    }
    const network = topics.includes("network");
    if (network !== this.watchingNetwork) {
      this.watchingNetwork = network;
      this.bridge.act({ kind: "watch_network", on: network }).catch(() => {});
    }
  }

  private async ask(action: Action): Promise<ActResult> {
    return result(await this.bridge.act(action));
  }

  get currentTopics(): Topic[] {
    return this.topics;
  }

  async applyScene(id: string): Promise<ActResult> {
    return result(await this.bridge.act({ kind: "apply_scene", id }));
  }

  async switchMonitor(monitor: string, input: MonitorInput): Promise<ActResult> {
    return result(await this.bridge.act({ kind: "switch_monitor", monitor, input }));
  }

  async saveScene(scene: SceneJson, isNew: boolean): Promise<ActResult> {
    const r = result(await this.bridge.act(isNew ? { kind: "add_scene", scene } : { kind: "edit_scene", scene }));
    return r.ok ? { ok: true, text: `Scene “${scene.name}” saved` } : r;
  }

  /** No command in the agent's IPC: `can("forget")` is false and the screen does not offer the action. */
  async forgetMonitor(): Promise<ActResult> {
    return { ok: false, reason: "The agent cannot forget a monitor yet" };
  }

  private probeListeners = new Set<(m: Machine) => void>();

  private txnListeners = new Set<(e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void>();
  private dragListeners = new Set<(s: DragStatus) => void>();
  /** The Tauri event cannot be unsubscribed: it is listened to once and distributed (transactions, cursor and drag). */
  private noticesBound: Promise<void> | null = null;

  private bindNotices(): Promise<void> {
    this.noticesBound ??= Promise.resolve(
      this.bridge.onNotice?.((n) => {
        if (n.kind === "txn") for (const l of this.txnListeners) l(n.event);
        else if (n.kind === "key_probe") {
          for (const m of [n.event.heard, n.event.second]) {
            if (m) for (const f of [...this.probeListeners]) f(m);
          }
        } else if (n.kind === "cursor" || n.kind === "crossed") for (const f of [...this.cursorListeners]) f(n);
        else if (n.kind === "drag") for (const l of this.dragListeners) l(n.status);
      }),
    );
    return this.noticesBound;
  }

  async watchTxn(onEvent: (e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void): Promise<() => void> {
    await this.bindNotices();
    this.txnListeners.add(onEvent);
    return () => {
      this.txnListeners.delete(onEvent);
    };
  }

  async watchDrag(onStatus: (s: DragStatus) => void): Promise<() => void> {
    await this.bindNotices();
    this.dragListeners.add(onStatus);
    return () => {
      this.dragListeners.delete(onStatus);
    };
  }

  async setPower(machine: Machine, on: boolean): Promise<ActResult> {
    const r = await this.bridge.act({ kind: "set_power", machine, on });
    if (r.kind === "accepted" || r.kind === "done") return { ok: true };
    return { ok: false, reason: "reason" in r ? r.reason : "The agent did not accept it" };
  }

  /** 3 s bandwidth test over `path` (by default, the path carrying the input). */
  async measureLink(path?: LinkKind): Promise<ActResult> {
    const peerLink = this.last.kind === "connected" && this.last.peer.kind === "paired" ? this.last.peer.link : null;
    const kind = path ?? primaryOf(this.network)?.kind ?? peerLink;
    if (!kind) return { ok: false, reason: "No link with the other machine" };
    const r = await this.bridge.act({ kind: "measure_link", path: kind });
    if (r.kind === "bandwidth") return { ok: true, text: measuredText(kind, gbpsOf(r.bytes_per_sec) ?? 0) };
    return { ok: false, reason: "reason" in r ? r.reason : "No answer from the other machine" };
  }

  setModule(machine: Machine, module: string, on: boolean): Promise<ActResult> {
    return this.ask({ kind: "set_module", machine, module, on });
  }

  rescan(machine: Machine): Promise<ActResult> {
    return this.ask({ kind: "rescan", machine });
  }

  setPrefs(p: { cmdCtrl: boolean; dragFiles: boolean }): Promise<ActResult> {
    return this.ask({ kind: "set_prefs", cmd_ctrl: p.cmdCtrl, drag_files: p.dragFiles });
  }

  setDeskName(name: string): Promise<ActResult> {
    return this.ask({ kind: "set_desk_name", name });
  }

  setWol(mac: string | null): Promise<ActResult> {
    return this.ask({ kind: "set_wol", mac });
  }

  setLockShortcut(lock: string[]): Promise<ActResult> {
    return this.ask({ kind: "set_lock_shortcut", lock });
  }

  async undoDesk(rev: number): Promise<ActResult> {
    const r = await this.ask({ kind: "undo_desk", to_rev: rev });
    return r.ok ? { ok: true, text: "Change undone" } : r;
  }

  async files(): Promise<FilesView> {
    return this.bridge.files();
  }

  async openFiles(target: "config" | "logs"): Promise<ActResult> {
    try {
      await this.bridge.openFiles(target);
      return { ok: true };
    } catch (e) {
      return { ok: false, reason: e instanceof Error ? e.message : String(e) };
    }
  }

  editMachine(machine: Machine, edit: { name?: string; color?: string }): Promise<ActResult> {
    return this.ask({ kind: "set_machine", machine, ...edit });
  }

  async setArrangement(a: Arrangement): Promise<ActResult> {
    const r = result(await this.bridge.act({ kind: "set_arrangement", a_side: a.aSide, align: a.align }));
    return r.ok ? { ok: true, text: "Arrangement saved" } : r;
  }

  async watchCursor(onEvent: (e: CursorEvent) => void): Promise<() => void> {
    if (typeof this.bridge.onNotice !== "function") throw new Error("The agent does not report the cursor yet");
    this.cursorListeners.add(onEvent);
    if (this.cursorListeners.size === 1) {
      const r = await this.bridge.act({ kind: "watch_cursor", on: true });
      if (r.kind === "rejected" || r.kind === "unavailable") {
        this.cursorListeners.delete(onEvent);
        throw new Error(r.reason);
      }
    }
    return () => {
      if (!this.cursorListeners.delete(onEvent) || this.cursorListeners.size > 0) return;
      this.bridge.act({ kind: "watch_cursor", on: false }).catch(() => {});
    };
  }

  async watchKeyProbe(onHeard: (m: Machine) => void): Promise<() => void> {
    await this.bindNotices();
    this.probeListeners.add(onHeard);
    if (this.probeListeners.size === 1) {
      const r = await this.bridge.act({ kind: "key_probe", on: true });
      if (r.kind === "rejected" || r.kind === "unavailable") {
        this.probeListeners.delete(onHeard);
        throw new Error(r.reason);
      }
    }
    return () => {
      if (!this.probeListeners.delete(onHeard) || this.probeListeners.size > 0) return;
      this.bridge.act({ kind: "key_probe", on: false }).catch(() => {});
    };
  }

  /**
   * The crossing test with what the engine reports (T-128): each `Crossed` is a real crossing. The
   * entry point comes from the first cursor sample already on the other machine; if it does not arrive
   * within [`CROSS_SETTLE_MS`], the crossing is reported without a position (`of: 0`) instead of inventing one.
   */
  async watchCrossing(onCross: (s: CrossSample) => void): Promise<() => void> {
    let pending: { from: Machine; to: Machine; edge: Side; t0: number; timer: ReturnType<typeof setTimeout> } | null = null;
    const emit = (p: NonNullable<typeof pending>, at: number, of: number, ms: number | null) => {
      clearTimeout(p.timer);
      if (pending === p) pending = null;
      onCross({ from: p.from, to: p.to, edge: p.edge, at, of, ms });
    };
    const stop = await this.watchCursor((e) => {
      if (e.kind === "crossed") {
        const t0 = performance.now();
        const p = { from: e.from, to: e.to, edge: e.edge, t0, timer: setTimeout(() => emit(p, 0, 0, null), CROSS_SETTLE_MS) };
        if (pending) clearTimeout(pending.timer);
        pending = p;
      } else if (pending && e.sample.machine === pending.to) {
        const p = pending;
        const layout = this.last.kind === "connected" ? layoutFromDesk(this.last) : EMPTY_LAYOUT;
        const mine = layout.screens.filter((s) => s.machine === p.to);
        const on = mine.find((s) => s.native?.id === e.sample.screen) ?? mine[0];
        const vertical = isVertical(p.edge);
        const len = vertical ? on?.native?.h : on?.native?.w;
        const start = vertical ? on?.native?.y : on?.native?.x;
        const at = (vertical ? e.sample.y : e.sample.x) - (start ?? 0);
        emit(p, len ? Math.round(at) : 0, len ?? 0, performance.now() - p.t0);
      }
    });
    return () => {
      if (pending) clearTimeout(pending.timer);
      pending = null;
      stop();
    };
  }

  async reportHeardKey(): Promise<void> {
    await this.bridge.act({ kind: "heard_key" });
  }

  async pairAction(kind: PairActionKind): Promise<ActResult> {
    return result(await this.bridge.act({ kind }));
  }

  async lockHere(on: boolean): Promise<ActResult> {
    const r = result(await this.bridge.act({ kind: "lock_here", on }));
    return r.ok ? { ok: true, text: on ? "Cursor locked here" : "Lock removed" } : r;
  }

  async requestPermissions(): Promise<ActResult> {
    return result(await this.bridge.act({ kind: "request_permissions" }));
  }

  async checkLocalNetwork(): Promise<ActResult> {
    const r = result(await this.bridge.act({ kind: "check_local_network" }));
    return r.ok ? { ok: true, text: "Tandem tried the local network again: if macOS asks, choose Allow" } : r;
  }

  async openPermission(p: Permission): Promise<ActResult> {
    await this.bridge.openSettings(p);
    return { ok: true };
  }

  async restartAgent(): Promise<ActResult> {
    await this.bridge.restartAgent();
    return { ok: true, text: "tandemd is restarting: permissions are re-checked in a few seconds" };
  }

  adminStatus(): Promise<AdminView> {
    return this.bridge.adminStatus();
  }

  installAdmin(): Promise<AdminView> {
    return this.bridge.installAdmin();
  }

  setup(): Promise<SetupView> {
    return this.bridge.setup();
  }

  async finishSetup(): Promise<void> {
    await this.bridge.finishSetup();
  }

  shortcuts(): Promise<ShortcutsView> {
    return this.bridge.shortcuts();
  }

  setShortcuts(b: Bindings): Promise<ShortcutsView> {
    return this.bridge.setShortcuts(b);
  }

  hideMain(): Promise<void> {
    return this.bridge.hideMain();
  }

  private updateListener: ((v: UpdateView) => void) | null = null;

  async updates(onChange: (v: UpdateView) => void): Promise<UpdateView> {
    // A single listener for the event: the tab can be opened and closed many times.
    if (this.updateListener === null) await this.bridge.onUpdates((v) => this.updateListener?.(v));
    this.updateListener = onChange;
    return this.bridge.updates();
  }

  checkUpdates(): Promise<UpdateView> {
    return this.bridge.checkUpdates();
  }

  updateAll(): Promise<UpdateView> {
    return this.bridge.updateAll();
  }

  setAutoUpdate(on: boolean): Promise<UpdateView> {
    return this.bridge.setAutoUpdate(on);
  }

  async processes(tunnel: string): Promise<ProcessRow[]> {
    const r = await this.bridge.act({ kind: "processes", tunnel });
    if (r.kind === "processes") return r.processes.map(processRow);
    throw new Error(r.kind === "rejected" || r.kind === "unavailable" ? r.reason : "the agent did not answer with processes");
  }

  async output(tunnel: string, process: number, from: OutputCursor): Promise<OutputChunk> {
    const r = await this.bridge.act({ kind: "output", tunnel, process, stdout: from.stdout, stderr: from.stderr });
    if (r.kind === "output") return outputChunk(r.output);
    throw new Error(r.kind === "rejected" || r.kind === "unavailable" ? r.reason : "the agent did not answer with the output");
  }

  can(what: Capability): boolean {
    if (what === "scenes" || what === "monitors") return this.last.kind === "connected";
    if (what === "power" || what === "measure") return this.last.kind === "connected" && this.last.peer.kind === "paired";
    if (what === "cursor" || what === "crossing") return this.last.kind === "connected" && this.last.peer.kind === "paired";
    if (what === "modules" || what === "machine" || what === "lock") return this.last.kind === "connected";
    return false;
  }

  stop(): void {}
}
