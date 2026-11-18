// La fuente real: el estado de tandemd por la IPC (a través de Tauri). Traduce el
// `Snapshot` del agente al `DeskView` de las pantallas.
//
// Hoy la IPC manda una foto completa con cada cambio: `watch` solo recuerda qué temas pide
// la pantalla visible. Los eventos por tema (recursos, túneles, módulos) llegan con
// T-098/T-102/T-104 y se suscriben aquí.

import { ASSUMED_PPI, DEFAULT_ARRANGEMENT, EMPTY_LAYOUT, ratioLabel, type Arrangement, type LayoutScreen, type LayoutView } from "../acomodo";
import type { Bridge } from "../bridge";
import { nextLive, summaryOf } from "../machine";
import { machineColor, machineName, other } from "../machines";
import type { Action, ActionResult, Bindings, Caps, Link, Machine, MachineCaps, MonitorInput, Permission, ResourceSample, SceneJson, SetupView, ShortcutsView, Snapshot, TxnView } from "../model";
import { gbpsOf, measuredText, networkFromWire, primaryOf } from "../network";
import { monitorsUi } from "../monitors";
import { sceneView } from "../scenes";
import { grantTo, permissionState } from "../permissions";
import type { ActResult, Capability, DeskView, EnvView, HardwareView, LinkKind, LiveView, MachineView, ModuleView, NetworkView, PairActionKind, PermissionsView, Source, Topic } from "./types";
import type { UpdateView } from "../updates";
import { EMPTY_NETWORK } from "./types";

/** Los recursos en vivo de cada lado, armados con las muestras del agente. */
export type LiveByMachine = Partial<Record<Machine, LiveView | null>>;

const MAC_PERMISSIONS: Permission[] = ["accessibility", "input_monitoring", "local_network"];

const OS_NAME = { macos: "macOS", windows: "Windows" } as const;

/** El nombre del puesto cuando no tiene uno propio. */
export const DEFAULT_DESK_NAME = "Mi puesto";

export const EMPTY_DESK: DeskView = {
  ready: false,
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
};

/** La foto del agente no tiene la forma que espera esta versión de la UI. */
export class ShapeError extends Error {
  constructor(what: string) {
    super(`el agente respondió algo que esta versión de la app no entiende (${what})`);
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
  if (!isObj(snap) || typeof snap.kind !== "string") throw new ShapeError("sin «kind»");
  if (snap.kind !== "connected") return;
  if (!isSide(snap.agent)) throw new ShapeError(`lado «${String(snap.agent)}»`);
  const desk = snap.desk;
  if (!isObj(desk) || !isObj(desk.machines) || !Array.isArray(desk.screens) || (desk.arrangement !== null && !isObj(desk.arrangement))) throw new ShapeError("puesto");
  if (!isObj(snap.state) || !isObj(snap.state.on) || typeof snap.state.on.a !== "boolean" || typeof snap.state.on.b !== "boolean") throw new ShapeError("encendido");
  if (!isObj(snap.peer) || !isObj(snap.pairing)) throw new ShapeError("emparejamiento");
  if (!Array.isArray(desk.scenes)) throw new ShapeError("escenas");
  if (!Array.isArray(snap.monitors)) throw new ShapeError("monitores");
  if (!isObj(snap.input) || !Array.isArray(snap.input.missing)) throw new ShapeError("entrada");
  const caps = snap.caps;
  if (!isObj(caps) || !isObj(caps.here) || !isSide(caps.here.machine) || !isObj(caps.here.input)) throw new ShapeError("capacidades");
  if (caps.peer !== null && (!isObj(caps.peer) || !isSide(caps.peer.machine))) throw new ShapeError("capacidades de la otra");
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
    live: shown,
  };
}

const LINK_KIND: Record<Link, DeskView["link"]["kind"]> = { usb4: "usb4", ethernet: "ethernet", wifi: "wifi" };

/** El resumen del camino que lleva la entrada; los números salen de la red si se está mirando. */
function linkOf(kind: DeskView["link"]["kind"], network: NetworkView): DeskView["link"] {
  const p = kind && primaryOf(network)?.kind === kind ? primaryOf(network) : null;
  return { kind, gbps: p?.gbps ?? null, ms: p?.rttMs ?? null, inputP99: p?.inputP99Ms ?? null };
}

/** El puesto que dibuja la UI a partir de la foto del agente. Puro; lanza `ShapeError`. */
export function deskFromSnapshot(raw: Snapshot, live: LiveByMachine = {}, network: NetworkView = EMPTY_NETWORK): DeskView {
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
    troubleText = "Faltan permisos en esta máquina";
  } else if (snap.peer.kind === "paired" && !snap.state.on[peer]) {
    trouble = "peer-off";
    troubleText = `${peerName} está apagada`;
  } else if (snap.peer.kind === "paired" && !linked) {
    trouble = "link-down";
    troubleText = `Sin enlace con ${peerName}`;
  }
  const monitors = monitorsUi(snap.monitors);
  // Los atajos se escriben según el sistema de esta máquina, no el del navegador.
  const mac = snap.caps.here.os === "macos";
  const name = (m: Machine) => machineName(m, snap.caps, snap.desk);
  return {
    ...EMPTY_DESK,
    ready: true,
    deskName: snap.desk.name?.trim() || DEFAULT_DESK_NAME,
    monitors,
    scenes: snap.desk.scenes.map((sc) => sceneView(sc, { monitors, name, mac })),
    activeScene: snap.state.scene ?? null,
    trouble,
    troubleText,
    here,
    machines,
    link: linkOf(snap.peer.kind === "paired" && snap.peer.link ? LINK_KIND[snap.peer.link] : null, network),
    network,
    pair: { peer: snap.peer, pairing: snap.pairing },
    permissions: permissionsOf(snap.caps.here),
    layout: layoutFromDesk(snap),
  };
}

/**
 * Las pantallas del acomodo según el puesto: las declaradas de cada máquina, o una por
 * máquina si no declaró ninguna. El agente todavía no informa el tamaño físico: se estima
 * con [`ASSUMED_PPI`] a partir de los puntos lógicos.
 */
export function layoutFromDesk(snap: Extract<Snapshot, { kind: "connected" }>): LayoutView {
  const screens: LayoutScreen[] = [];
  for (const m of ["a", "b"] as Machine[]) {
    const name = machineName(m, snap.caps, snap.desk);
    const known = snap.desk.machines?.[m] ?? (snap.caps.here.machine === m ? snap.caps.here : snap.caps.peer?.machine === m ? snap.caps.peer : null);
    if (!known) continue;
    const mac = known.os === "macos";
    const own = snap.desk.screens.filter((s) => s.machine === m);
    if (own.length === 0) {
      screens.push({ id: `escritorio-${m}`, machine: m, name, short: name.split(" ")[0] ?? name, ask: name, x: 0, y: 0, w: 16, h: 10, size: "—", res: "—", ratio: "—", scale: "—", usable: true, line: name });
      continue;
    }
    // Las posiciones (`x`, `y`) van en las coordenadas de la máquina: se pasan a puntos con la
    // escala de su primera pantalla. Es aproximado si las escalas de sus monitores difieren.
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

  constructor(private readonly bridge: Bridge) {}

  async start(onDesk: (d: DeskView) => void): Promise<void> {
    // Un error al leer una foto no cuelga la ventana: se muestra como «sin agente» con la razón.
    const push = (s: Snapshot) => {
      try {
        const d = deskFromSnapshot(s, this.live, s.kind === "connected" ? this.network : EMPTY_NETWORK);
        this.last = s;
        onDesk(d);
      } catch (e) {
        this.last = { kind: "connecting" };
        onDesk(deskFromError(e));
      }
    };
    await this.bridge.onSnapshot(push);
    // Las muestras llegan aparte (una por segundo): cada una avanza la línea de su máquina.
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
    return r.ok ? { ok: true, text: `Escena «${scene.name}» guardada` } : r;
  }

  /** Sin comando en la IPC del agente: `can("forget")` es falso y la pantalla no ofrece la acción. */
  async forgetMonitor(): Promise<ActResult> {
    return { ok: false, reason: "El agente todavía no sabe olvidar un monitor" };
  }

  private txnListeners = new Set<(e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void>();
  private txnBound: Promise<void> | null = null;

  async watchTxn(onEvent: (e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void): Promise<() => void> {
    // El evento de Tauri no se puede dejar de oír: se oye una vez y se reparte.
    this.txnBound ??= this.bridge.onNotice((n) => {
      if (n.kind === "txn") for (const l of this.txnListeners) l(n.event);
    });
    await this.txnBound;
    this.txnListeners.add(onEvent);
    return () => {
      this.txnListeners.delete(onEvent);
    };
  }

  async setPower(machine: Machine, on: boolean): Promise<ActResult> {
    const r = await this.bridge.act({ kind: "set_power", machine, on });
    if (r.kind === "accepted" || r.kind === "done") return { ok: true };
    return { ok: false, reason: "reason" in r ? r.reason : "El agente no lo aceptó" };
  }

  /** Prueba de ancho de banda de 3 s por `path` (por omisión, el camino que lleva la entrada). */
  async measureLink(path?: LinkKind): Promise<ActResult> {
    const peerLink = this.last.kind === "connected" && this.last.peer.kind === "paired" ? this.last.peer.link : null;
    const kind = path ?? primaryOf(this.network)?.kind ?? peerLink;
    if (!kind) return { ok: false, reason: "Sin enlace con la otra máquina" };
    const r = await this.bridge.act({ kind: "measure_link", path: kind });
    if (r.kind === "bandwidth") return { ok: true, text: measuredText(kind, gbpsOf(r.bytes_per_sec) ?? 0) };
    return { ok: false, reason: "reason" in r ? r.reason : "Sin respuesta de la otra máquina" };
  }

  setModule(machine: Machine, module: string, on: boolean): Promise<ActResult> {
    return this.ask({ kind: "set_module", machine, module, on });
  }

  rescan(machine: Machine): Promise<ActResult> {
    return this.ask({ kind: "rescan", machine });
  }

  editMachine(machine: Machine, edit: { name?: string; color?: string }): Promise<ActResult> {
    return this.ask({ kind: "set_machine", machine, ...edit });
  }

  async setArrangement(a: Arrangement): Promise<ActResult> {
    const r = result(await this.bridge.act({ kind: "set_arrangement", a_side: a.aSide, align: a.align }));
    return r.ok ? { ok: true, text: "Acomodo guardado" } : r;
  }

  async watchCrossing(): Promise<() => void> {
    // El motor de entrada todavía no informa los cruces por la IPC (`CursorSample`).
    throw new Error("El agente todavía no informa el cruce del cursor");
  }

  async pairAction(kind: PairActionKind): Promise<ActResult> {
    return result(await this.bridge.act({ kind }));
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
    return { ok: true, text: "tandemd se reinicia: en unos segundos se vuelven a comprobar los permisos" };
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

  can(what: Capability): boolean {
    if (what === "scenes" || what === "monitors") return this.last.kind === "connected";
    if (what === "power" || what === "measure") return this.last.kind === "connected" && this.last.peer.kind === "paired";
    if (what === "modules" || what === "machine") return this.last.kind === "connected";
    return false;
  }

  stop(): void {}
}
