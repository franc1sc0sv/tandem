// La fuente real: el estado de tandemd por la IPC (a través de Tauri). Traduce el
// `Snapshot` del agente al `DeskView` de las pantallas.
//
// Hoy la IPC manda una foto completa con cada cambio: `watch` solo recuerda qué temas pide
// la pantalla visible. Los eventos por tema (recursos, tunnels, módulos) llegan con
// T-098/T-102/T-104 y se suscriben aquí.

import { ASSUMED_PPI, DEFAULT_ARRANGEMENT, EMPTY_LAYOUT, isVertical, ratioLabel, type Arrangement, type LayoutScreen, type LayoutView } from "../arrangement";
import type { Bridge } from "../bridge";
import type { CrossSample } from "../crossing";
import { nextLive, summaryOf } from "../machine";
import { machineColor, machineName, other } from "../machines";
import type { Action, ActionResult, AdminView, Bindings, Caps, Desk, DragStatus, Link, Machine, MachineCaps, MonitorInput, Side, Permission, ResourceSample, SceneJson, SetupView, ShortcutsView, Snapshot, TxnView } from "../model";
import { devicesOfHub } from "../wizard";
import { gbpsOf, measuredText, networkFromWire, primaryOf } from "../network";
import { monitorsUi } from "../monitors";
import { sceneView } from "../scenes";
import { grantTo, permissionState } from "../permissions";
import type { ActResult, Capability, CursorEvent, DeskChange, DeskView, FilesView, EnvView, HardwareView, LinkKind, LiveView, MachineView, ModuleView, NetworkView, OutputChunk, OutputCursor, PairActionKind, PermissionsView, PrefsView, ProcessRow, Source, Topic, TransferView, TunnelView } from "./types";
import { addTransfer, newMemory, outputChunk, processRow, tunnelsFromWire, type Memory, type ProjectWire } from "../tunnels";
import type { UpdateView } from "../updates";
import { EMPTY_NETWORK } from "./types";

/** Los recursos en vivo de cada lado, armados con las muestras del agente. */
export type LiveByMachine = Partial<Record<Machine, LiveView | null>>;

/** Lo que se espera a la primera muestra del cursor tras un crossing antes de informarlo sin posición. */
const CROSS_SETTLE_MS = 250;

const MAC_PERMISSIONS: Permission[] = ["accessibility", "input_monitoring", "local_network"];

const OS_NAME = { macos: "macOS", windows: "Windows" } as const;

/** El nombre del desk cuando no tiene uno propio. */
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

/** La foto del agente no tiene la forma que espera esta versión de la UI. */
export class ShapeError extends Error {
  constructor(what: string) {
    super(`the agent replied with something this version of the app does not understand (${what})`);
    this.name = "ShapeError";
  }
}

const isSide = (m: unknown): m is Machine => m === "a" || m === "b";
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

/**
 * Comprueba lo que la UI lee de una foto conectada. Un agente de otra versión (por ejemplo
 * uno con lados `mac`/`pc`) da un estado difícil con la razón, no una ventana colgada.
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

/** Keyboard and mouse según el desk; lo que el agente no diga vale «encendido» (como en su `Default`). */
export function prefsOf(desk: Desk): PrefsView {
  const on = (m: Machine) => desk.modules?.[m]?.clipboard !== false;
  return { cmdCtrl: desk.prefs?.cmd_ctrl ?? true, dragFiles: desk.prefs?.drag_files ?? true, clipboard: on("a") && on("b") };
}

function permissionsOf(here: MachineCaps): PermissionsView {
  if (here.os !== "macos") return { needed: false, rows: [], binary: null };
  return { needed: true, rows: MAC_PERMISSIONS.map((id) => ({ id, state: permissionState(here, id) })), binary: grantTo(here) };
}

function modulesOf(c: MachineCaps | null): ModuleView[] {
  // Si no está disponible, la línea dice por qué (T-098: razones accionables).
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

/** El resumen del camino que lleva la entrada; los números salen de la red si se está mirando. */
function linkOf(kind: DeskView["link"]["kind"], network: NetworkView): DeskView["link"] {
  const p = kind && primaryOf(network)?.kind === kind ? primaryOf(network) : null;
  return { kind, gbps: p?.gbps ?? null, ms: p?.rttMs ?? null, inputP99: p?.inputP99Ms ?? null };
}

/** El desk que dibuja la UI a partir de la foto del agente. Puro; lanza `ShapeError`. */
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
  // Los shortcuts se escriben según el sistema de this machine, no el del navegador.
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
 * Las pantallas del arrangement según el desk: las declaradas de cada machine, o una por
 * machine si no declaró ninguna. El agente todavía no informa el tamaño físico: se estima
 * con [`ASSUMED_PPI`] a partir de los puntos lógicos.
 */
export function layoutFromDesk(snap: Extract<Snapshot, { kind: "connected" }>): LayoutView {
  const screens: LayoutScreen[] = [];
  for (const m of ["a", "b"] as Machine[]) {
    const name = machineName(m, snap.caps, snap.desk);
    const known = snap.desk.machines?.[m] ?? (snap.caps.here.machine === m ? snap.caps.here : snap.caps.peer?.machine === m ? snap.caps.peer : null);
    if (!known) continue;
    const mac = known.os === "macos";
    const declared = snap.desk.screens.filter((s) => s.machine === m);
    // Sin `[[screens]]` (lo normal tras emparejar) se usan las que lee el motor de cada sistema.
    const engine = (snap.agent === m ? snap.input.screens : snap.input.peer_screens) ?? [];
    const own = declared.length > 0 ? declared : engine.map((s) => ({ id: s.id, x: s.x, y: s.y, width: s.width, height: s.height, scale: s.scale_milli / 1000 }));
    if (own.length === 0) {
      screens.push({ id: `desk-${m}`, machine: m, name, short: name.split(" ")[0] ?? name, ask: name, x: 0, y: 0, w: 16, h: 10, size: "—", res: "—", ratio: "—", scale: "—", usable: true, line: name });
      continue;
    }
    // Las posiciones (`x`, `y`) van en las coordenadas de la machine: se pasan a puntos con la
    // escala de su primera pantalla. Es aproximado si las escalas de sus monitors difieren.
    const base = mac ? 1 : own[0]?.scale || 1;
    for (const s of own) {
      // macOS da puntos; Windows, píxeles físicos que la escala convierte en lógicos.
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

/** El estado difícil de un error al hablar con el agente. */
export function deskFromError(e: unknown): DeskView {
  return { ...EMPTY_DESK, ready: true, trouble: "no-agent", troubleText: e instanceof Error ? e.message : String(e) };
}

function result(r: ActionResult): ActResult {
  if (r.kind === "rejected" || r.kind === "unavailable") return { ok: false, reason: r.reason };
  return { ok: true };
}

export class LiveSource implements Source {
  private topics: Topic[] = [];
  private last: Snapshot = { kind: "connecting" };
  private live: LiveByMachine = {};
  /** Se le pidieron al agente los recursos en vivo. */
  private watching = false;
  private network: NetworkView = EMPTY_NETWORK;
  /** Se le pidió al agente la red en vivo. */
  private watchingNetwork = false;
  /** Los tunnels como los mandó el agente por última vez, y lo que la ventana recuerda de ellos. */
  private groups: ProjectWire[] = [];
  private readonly memory: Memory = newMemory();
  private tunnels: TunnelView[] = [];
  /** Ya llegaron tunnels (por evento o por pedido): un pedido tardío no pisa lo más fresh. */
  private gotTunnels = false;
  private askedTunnels = false;
  /** Quién recibe el cursor en vivo; el agente solo lo manda mientras haya alguien. */
  private readonly cursorListeners = new Set<(e: CursorEvent) => void>();

  constructor(private readonly bridge: Bridge) {}

  async start(onDesk: (d: DeskView) => void, onChange?: (c: DeskChange) => void, onTransfer?: (t: TransferView) => void): Promise<void> {
    // Un error al leer una foto no cuelga la ventana: se muestra como «no agent» con la razón.
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
    // La sesión solo empuja los tunnels al conectar y con cada cambio: una ventana que se abre
    // o recarga después los pide una vez (T-154). Cualquier otra respuesta se ignora.
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
    // El notice «Cambiado desde X · Deshacer» (D5): solo viaja por los notices, no por la foto.
    void Promise.resolve(
      this.bridge.onNotice?.((n) => {
        if (n.kind === "desk_changed") onChange?.({ by: n.by, rev: n.rev, summary: n.summary, undoTo: n.undo_to });
        else if (n.kind === "transfer") onTransfer?.({ upload: n.upload, path: n.path, done: n.done, total: n.total, finished: n.finished });
      }),
    ).catch(() => {});
    // Las muestras llegan aparte (una por segundo): cada una avanza la línea de su machine.
    // No se espera la suscripción: la foto no debe depender de que ese canal responda.
    void Promise.resolve(
      this.bridge.onResources?.((machine: Machine, r: ResourceSample) => {
        this.live = { ...this.live, [machine]: nextLive(this.live[machine] ?? null, r) };
        if (this.last.kind === "connected") push(this.last);
      }),
    ).catch(() => {});
    // Lo mismo con la red: cada estado se pinta con la última foto.
    void Promise.resolve(
      this.bridge.onNetwork?.((w) => {
        this.network = networkFromWire(w, Date.now());
        if (this.last.kind === "connected") push(this.last);
      }),
    ).catch(() => {});
    // Los tunnels llegan al conectar y con cada cambio, sin pedirlos; el avance de las copias
    // alimenta el tráfico de cada uno.
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

  /** Los recursos y la red solo se piden mientras una pantalla visible los muestra. */
  watch(topics: Topic[]): void {
    this.topics = [...topics];
    if (typeof this.bridge.act !== "function") return;
    const resources = topics.includes("resources");
    if (resources !== this.watching) {
      this.watching = resources;
      // La sesión recuerda el pedido y lo repite al reconectar; si falla ahora, no hay agente.
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

  /** Sin comando en la IPC del agente: `can("forget")` es falso y la pantalla no ofrece la acción. */
  async forgetMonitor(): Promise<ActResult> {
    return { ok: false, reason: "The agent cannot forget a monitor yet" };
  }

  private probeListeners = new Set<(m: Machine) => void>();

  private txnListeners = new Set<(e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void>();
  private dragListeners = new Set<(s: DragStatus) => void>();
  /** El evento de Tauri no se puede dejar de oír: se oye una vez y se reparte (transacciones, cursor y arrastre). */
  private noticesBound: Promise<void> | null = null;

  private bindNotices(): Promise<void> {
    this.noticesBound ??= Promise.resolve(
      this.bridge.onNotice?.((n) => {
        if (n.kind === "txn") for (const l of this.txnListeners) l(n.event);
        else if (n.kind === "key_probe") {
          if (n.event.heard) for (const f of [...this.probeListeners]) f(n.event.heard);
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

  /** Prueba de ancho de banda de 3 s por `path` (por omisión, el camino que lleva la entrada). */
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
   * La prueba de crossing con lo que informa el motor (T-128): cada `Crossed` es un crossing real. El
   * punto de entrada sale de la primera muestra del cursor ya en the other machine; si no llega
   * en [`CROSS_SETTLE_MS`], se informa el crossing sin posición (`of: 0`) en vez de inventarla.
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
    // Un solo oyente del evento: la pestaña puede abrirse y cerrarse muchas veces.
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
