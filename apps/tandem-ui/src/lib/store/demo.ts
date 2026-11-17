// Modo demo: un puesto simulado y coherente para cada pantalla y cada estado. Lo usan las
// capturas (`pnpm shots`), las pruebas y `pnpm dev` (`/?demo`). Los datos son los de los
// mockups de ejemplo; en la app real todo sale del escaneo de cada máquina (D10).
//
// Parámetros de la URL:
// - `demo` o `demo=mac-win` (por omisión), `demo=win-win`, `demo=mac-mac`: qué sistemas hay
//   en cada lado (D2).
// - `here=b`: la ventana es la de la máquina B.
// - `trouble=no-agent|peer-off|link-down|permissions`: un estado difícil (`V3-Estados`).
// - `still`: sin recursos que cambien solos (para capturas deterministas).
// - `peer=none`: sin emparejar; emparejar simula a la otra máquina (código y confirmación).
//   `pair=code` arranca mostrando el código; `pair=reject`: la otra máquina rechaza.
// - `cross=ok|wrong|timeout`: la prueba de cruce cruza por el borde del acomodo, por otro,
//   o se queda sin tiempo apenas empieza (para las capturas). Sin esto (y sin `still`), cruza
//   bien a los pocos segundos.

import { exitSide, isVertical, type Arrangement, type LayoutScreen, type LayoutView } from "../acomodo";
import type { CrossSample } from "../crossing";
import type { Bindings, PairingView, Permission, SetupView, ShortcutsView } from "../model";
import { DEFAULT_BINDINGS } from "../shortcuts";
import type { UpdateView } from "../updates";
import { EMPTY_NETWORK, INPUT_TARGET_MS } from "./types";
import type {
  ActResult,
  DeskView,
  LatencyPoint,
  LinkKind,
  Machine,
  MachineView,
  ModuleView,
  NetworkView,
  Os,
  PairActionKind,
  PathView,
  PermissionsView,
  Resources,
  SceneView,
  Source,
  Topic,
  Trouble,
  TunnelView,
} from "./types";

export type DemoPair = "mac-win" | "win-win" | "mac-mac";

export interface DemoOptions {
  pair: DemoPair;
  here: Machine;
  trouble: Trouble;
  still: boolean;
  /** Arranca sin emparejar. */
  unpaired?: boolean;
  /** Cómo responde la otra máquina al emparejar. */
  pairing?: "code" | "reject";
  /** La guía de permisos del primer arranque se abre sola. */
  firstRun?: boolean;
  /** Cómo termina la prueba de cruce. */
  cross?: "ok" | "wrong" | "timeout";
}

export function demoOptions(query: string): DemoOptions {
  const p = new URLSearchParams(query);
  const pair = p.get("demo");
  const trouble = p.get("trouble");
  return {
    pair: pair === "win-win" || pair === "mac-mac" ? pair : "mac-win",
    here: p.get("here") === "b" ? "b" : "a",
    trouble: trouble === "no-agent" || trouble === "peer-off" || trouble === "link-down" || trouble === "permissions" ? trouble : "none",
    still: p.has("still"),
    unpaired: p.get("peer") === "none",
    pairing: p.get("pair") === "code" ? "code" : p.get("pair") === "reject" ? "reject" : undefined,
    firstRun: p.has("setup"),
    cross: p.get("cross") === "ok" ? "ok" : p.get("cross") === "wrong" ? "wrong" : p.get("cross") === "timeout" ? "timeout" : undefined,
  };
}

export function isDemo(query: string): boolean {
  return new URLSearchParams(query).has("demo");
}

const BLUE = "#7aa7e6";
const VIOLET = "#a99be8";

/** Una pantalla de ejemplo (datos del mockup `V3-Acomodo`). */
interface DemoScreen {
  id: string;
  name: string;
  short: string;
  ask: string;
  w: number;
  h: number;
  size: string;
  res: string;
  ratio: string;
  scale: string;
  /** Monitor externo: lo que muestra depende de la escena. */
  external?: boolean;
  /** Desconectado: no entra en el acomodo. */
  off?: boolean;
}

interface Spec {
  name: string;
  os: Os;
  color: string;
  detail: string;
  resources: Resources;
  screens: DemoScreen[];
}

const BUILTIN_14: DemoScreen = { id: "mbp", name: "MacBook Pro", short: "Mac", ask: "MacBook Pro", w: 12, h: 7.8, size: '14"', res: "1512 × 982", ratio: "16:10", scale: "200 % (Retina)" };
const G34: DemoScreen = { id: "g34", name: 'Odyssey G5 34"', short: '34"', ask: 'el 34"', w: 31.4, h: 13.1, size: '34"', res: "3440 × 1440", ratio: "21:9", scale: "100 %", external: true };
const G32: DemoScreen = { id: "g32", name: 'Odyssey G5 32"', short: '32"', ask: 'el 32"', w: 27.9, h: 15.7, size: '32"', res: "2560 × 1440", ratio: "16:9", scale: "—", external: true, off: true };
const U27: DemoScreen = { id: "u27", name: 'UltraFine 27"', short: '27"', ask: 'el 27"', w: 23.5, h: 13.2, size: '27"', res: "2560 × 1440", ratio: "16:9", scale: "100 %", external: true };
const STUDIO: DemoScreen = { id: "sd27", name: "Studio Display", short: "Studio", ask: "el Studio Display", w: 23.5, h: 13.2, size: '27"', res: "2560 × 1440", ratio: "16:9", scale: "200 %", external: true };

const MACBOOK: Spec = {
  name: "MacBook Pro",
  os: "macos",
  color: BLUE,
  detail: "macOS · M5",
  resources: {
    cpu: { percent: 18, label: "18 %" },
    ram: { percent: 52, label: "12.4/24 GB" },
    gpu: { percent: 6, label: "6 %" },
    gpuLabel: "GPU",
    history: [26, 30, 22, 37, 32, 44, 33, 39, 28, 43, 35, 41, 33],
  },
  screens: [BUILTIN_14],
};

const DESKTOP: Spec = {
  name: "PC-Escritorio",
  os: "windows",
  color: VIOLET,
  detail: "Windows + WSL · RTX 5080",
  resources: {
    cpu: { percent: 64, label: "64 %" },
    ram: { percent: 71, label: "17.1/24 GB" },
    gpu: { percent: 88, label: "14.1/16 GB" },
    gpuLabel: "VRAM",
    history: [44, 52, 59, 67, 63, 74, 70, 78, 72, 81, 76, 80, 78],
  },
  screens: [G34, G32],
};

const WORKSHOP: Spec = {
  ...DESKTOP,
  name: "PC-Taller",
  color: "#6cc59a",
  detail: "Windows · Ryzen 7",
  resources: { ...DESKTOP.resources, gpu: null, gpuLabel: "GPU", cpu: { percent: 31, label: "31 %" } },
  screens: [U27],
};

const MINI: Spec = {
  ...MACBOOK,
  name: "Mac mini",
  color: "#d9a75a",
  detail: "macOS · M4",
  resources: { ...MACBOOK.resources, cpu: { percent: 9, label: "9 %" } },
  screens: [STUDIO],
};

const PAIRS: Record<DemoPair, [Spec, Spec]> = {
  "mac-win": [MACBOOK, DESKTOP],
  "win-win": [DESKTOP, WORKSHOP],
  "mac-mac": [MACBOOK, MINI],
};

function modulesOf(os: Os, otherName: string, permissions: boolean): ModuleView[] {
  const mac = os === "macos";
  const list: ModuleView[] = [
    mac && permissions
      ? { id: "input", name: "Teclado y mouse", state: "unavailable", detail: "Falta el permiso de Monitorización de entrada" }
      : { id: "input", name: "Teclado y mouse", state: "active", detail: mac ? "Captura con CGEventTap e inyección con CGEventPost" : "Captura e inyección (hooks de bajo nivel, SendInput)" },
    { id: "clipboard", name: "Portapapeles", state: "active", detail: "Texto, HTML e imagen" },
    { id: "files", name: "Archivos", state: "active", detail: "Arrastrar al cruzar el borde" },
    { id: "monitors", name: "Monitores", state: mac ? "off" : "active", detail: mac ? `Hoy los controla ${otherName}` : "Canal DDC por DisplayPort" },
    { id: "power", name: "Energía", state: "active", detail: mac ? "Despierta a la otra por Wake-on-LAN" : `Se apaga sola. Se despierta por Wake-on-LAN desde ${otherName}.` },
    { id: "tunnels", name: "Túneles", state: "active", detail: mac ? "2 entornos" : "3 entornos, como administrador" },
  ];
  if (!mac) list.push({ id: "secure-desktop", name: "Pantallas protegidas", state: "active", detail: "También en avisos de UAC y en el inicio de sesión" });
  return list;
}

export const DEMO_SCENES = (a: string, b: string): SceneView[] => [
  { id: "trabajo", name: "Trabajo", key: "⌃⌥1", desc: `34" muestra ${b}`, shows: "b" },
  { id: "solo-a", name: `Solo ${a.split(" ")[0]}`, key: "⌃⌥2", desc: `34" muestra ${a}`, shows: "a" },
  { id: "juego", name: "Juego", key: "⌃⌥3", desc: `34" muestra ${b}`, shows: "b" },
];

export const DEMO_TUNNELS: TunnelView[] = [
  { id: "t97", project: "tandem", title: "Crate de monitores (T-097)", from: "a", to: "b", state: "busy", processes: 6 },
  { id: "t101", project: "tandem", title: "Túneles en tandemd (T-101)", from: "a", to: "b", state: "busy", processes: 2 },
  { id: "pr", project: "tandem", title: "Revisar PR #66 en Windows", from: "b", to: "a", state: "idle", processes: 0 },
  { id: "lora", project: "ia-local", title: "Entrenar LoRA de estilo", from: "a", to: "b", state: "busy", processes: 3 },
  { id: "comfy", project: "ia-local", title: "Probar flujo de ComfyUI", from: "a", to: "b", state: "closed", processes: 0 },
];

/** La hora del demo (fija: capturas deterministas). Los cambios de camino se fechan desde aquí. */
export const DEMO_NOW = 1_763_400_000_000;

/** Lo que da cada camino en el demo, en Gbps (los números de `V3-Red`). */
const DEMO_GBPS: Record<LinkKind, number> = { usb4: 9.4, ethernet: 2.3, wifi: 0.41 };

const HISTORY = 60;
/** Segundo del historial en que se desconectó el cable y en que volvió. */
const CUT_AT = 34;
const BACK_AT = 41;

const round = (n: number, d = 10) => Math.round(n * d) / d;

/**
 * El segundo `i` del historial de un camino (determinista). El USB-C lleva la entrada salvo
 * entre el corte y la vuelta, cuando la lleva el Wi-Fi: ahí está el pico de la gráfica.
 */
function demoPoint(kind: LinkKind, i: number): LatencyPoint {
  const jitter = ((i * 7) % 5) / 5;
  if (kind === "ethernet") return { rttMs: null, inputP99Ms: null };
  if (kind === "usb4") {
    if (i >= CUT_AT && i < BACK_AT) return { rttMs: null, inputP99Ms: null };
    return { rttMs: round(0.3 + jitter * 0.1, 100), inputP99Ms: round(1.6 + (((i * 5) % 7) / 7) * 0.6, 100) };
  }
  const carrying = i >= CUT_AT && i < BACK_AT;
  // El pico: la primera tecla después del corte esperó el cambio de camino.
  const input = !carrying ? null : i === CUT_AT ? 14.2 : round(6.5 + jitter * 2);
  return { rttMs: round(3.6 + jitter * 0.6, 100), inputP99Ms: input };
}

/** La red del demo: USB-C principal, Ethernet sin cable y Wi-Fi de respaldo (`V3-Red`). */
export function demoNetwork(linkUp: boolean): NetworkView {
  if (!linkUp) return { ...EMPTY_NETWORK, paths: EMPTY_NETWORK.paths.map((p) => ({ ...p })) };
  const path = (kind: LinkKind, state: PathView["state"], rttMs: number | null, input: [number, number] | null, gbps: number | null): PathView => {
    const history = Array.from({ length: HISTORY }, (_, i) => demoPoint(kind, i));
    return { kind, state, rttMs, inputP50Ms: input?.[0] ?? null, inputP99Ms: input?.[1] ?? null, targetMs: INPUT_TARGET_MS[kind], gbps, history };
  };
  const at = (i: number) => DEMO_NOW - (HISTORY - 1 - i) * 1000;
  return {
    paths: [path("usb4", "primary", 0.3, [0.4, 1.9], DEMO_GBPS.usb4), path("ethernet", "unplugged", null, null, null), path("wifi", "backup", 3.8, null, DEMO_GBPS.wifi)],
    switches: [
      { at: at(BACK_AT), from: "wifi", to: "usb4", tookMs: null },
      { at: at(CUT_AT), from: "usb4", to: "wifi", tookMs: 600 },
    ],
    measuring: null,
  };
}

const TROUBLE_TEXT: Record<Trouble, (peer: string) => string> = {
  none: () => "",
  "no-agent": () => "El agente de Tandem no está corriendo en esta máquina",
  "peer-off": (peer) => `${peer} está apagada`,
  "link-down": (peer) => `Sin enlace con ${peer}`,
  permissions: () => "Faltan permisos en esta máquina",
};

export const DEMO_CODE = "418027";

function permissionsFor(os: Os, missing: boolean): PermissionsView {
  if (os !== "macos") return { needed: false, rows: [], binary: null };
  return {
    needed: true,
    rows: [
      { id: "accessibility", state: missing ? "missing" : "granted" },
      { id: "input_monitoring", state: missing ? "missing" : "granted" },
      { id: "local_network", state: "unknown" },
    ],
    binary: "Concede el permiso a este binario: /Applications/Tandem.app/Contents/MacOS/tandemd (pid 4242)",
  };
}

/** Como en el mockup: la otra máquina a la izquierda del 34", alineada abajo. */
export const DEMO_ARRANGEMENT: Arrangement = { aSide: "left", align: "end" };

/** Las pantallas de ejemplo: las de A primero (como el mockup), las desconectadas al final. */
function demoLayout(sa: Spec, sb: Spec, scenes: SceneView[], active: string | null): LayoutView {
  const scene = scenes.find((x) => x.id === active) ?? null;
  const screens: LayoutScreen[] = [];
  const off: LayoutScreen[] = [];
  for (const [m, spec] of [["a", sa] as const, ["b", sb] as const]) {
    for (const d of spec.screens) {
      const { external, off: gone, ...rest } = d;
      const id = sa.screens.includes(d) && sb.screens.includes(d) ? `${m}-${d.id}` : d.id;
      if (gone) {
        off.push({ ...rest, id, machine: null, usable: false, line: "Desconectado" });
        continue;
      }
      const shows = external && scene?.shows === m ? ` · escena ${scene.name}` : "";
      screens.push({ ...rest, id, machine: m, usable: true, line: `${spec.name}${shows}` });
    }
  }
  return { screens: [...screens, ...off], arrangement: { ...DEMO_ARRANGEMENT } };
}

/** El puesto simulado para unas opciones (puro: mismas opciones, mismo puesto). */
export function demoDesk(o: DemoOptions): DeskView {
  const [sa, sb] = PAIRS[o.pair];
  const peer: Machine = o.here === "a" ? "b" : "a";
  const peerSpec = peer === "a" ? sa : sb;
  const mk = (id: Machine, s: Spec, otherName: string): MachineView => {
    const here = id === o.here;
    const power = !here && o.trouble === "peer-off" ? "off" : !here && o.trouble === "link-down" ? "lost" : "on";
    return {
      id,
      name: s.name,
      os: s.os,
      color: s.color,
      detail: s.detail,
      here,
      power,
      resources: power === "on" ? { ...s.resources, history: [...s.resources.history] } : null,
      modules: modulesOf(s.os, otherName, here && o.trouble === "permissions"),
    };
  };
  const scenes = DEMO_SCENES(sa.name, sb.name);
  const linkUp = o.trouble !== "peer-off" && o.trouble !== "link-down" && o.trouble !== "no-agent";
  return {
    ready: true,
    trouble: o.trouble,
    troubleText: TROUBLE_TEXT[o.trouble](peerSpec.name),
    here: o.here,
    machines: { a: mk("a", sa, sb.name), b: mk("b", sb, sa.name) },
    link: linkUp ? { kind: "usb4", gbps: 9.4, ms: 0.3, inputP99: 1.9 } : { kind: null, gbps: null, ms: null, inputP99: null },
    network: demoNetwork(linkUp),
    tunnels: DEMO_TUNNELS.map((t) => ({ ...t })),
    scenes,
    activeScene: "trabajo",
    pair: {
      peer: o.unpaired ? { kind: "unpaired" } : { kind: "paired", name: peerSpec.name, link: linkUp ? "usb4" : null },
      pairing: o.pairing === "code" ? { kind: "comparing", code: DEMO_CODE, peer: peerSpec.name, confirmed: false } : { kind: "idle" },
    },
    permissions: permissionsFor((o.here === "a" ? sa : sb).os, o.trouble === "permissions"),
    layout: demoLayout(sa, sb, scenes, "trabajo"),
  };
}

/** Cuánto tarda el cruce simulado cuando nadie dijo cómo termina. */
const CROSS_MS = 2500;

const PAIR_FOUND_MS = 1400;
const PAIR_OTHER_MS = 1200;

const TICK_MS = 1500;

/** Una fuente simulada: responde a las acciones como lo haría el agente. */
export class DemoSource implements Source {
  private desk: DeskView;
  private emit: (d: DeskView) => void = () => {};
  private timer: ReturnType<typeof setInterval> | undefined;
  private topics: Topic[] = [];
  /** Lo que se pidió en vivo, para las pruebas. */
  readonly watched: Topic[][] = [];

  constructor(private readonly opts: DemoOptions) {
    this.desk = demoDesk(opts);
  }

  async start(onDesk: (d: DeskView) => void): Promise<void> {
    this.emit = onDesk;
    onDesk(this.desk);
  }

  watch(topics: Topic[]): void {
    this.topics = [...topics];
    this.watched.push(this.topics);
    clearInterval(this.timer);
    this.timer = undefined;
    // Los recursos y la red cambian solos, y solo si la pantalla visible los muestra.
    const live = topics.includes("resources") || topics.includes("network");
    if (!this.opts.still && live) this.timer = setInterval(() => this.tick(), TICK_MS);
  }

  /** Avanza una muestra lo que la pantalla visible muestra: recursos y red. */
  tick(): void {
    if (this.topics.includes("resources")) this.tickResources();
    if (this.topics.includes("network")) this.tickNetwork();
  }

  /** El historial de cada camino que responde avanza un segundo. */
  private tickNetwork(): void {
    const paths = this.desk.network.paths.map((p) => {
      if (p.state !== "primary" && p.state !== "backup") return p;
      // Tráfico normal (los segundos de antes del corte), en ciclo.
      const point = demoPoint(p.kind, this.ticks % CUT_AT);
      return { ...p, rttMs: point.rttMs ?? p.rttMs, history: [...p.history.slice(1), point] };
    });
    this.ticks += 1;
    this.set({ ...this.desk, network: { ...this.desk.network, paths } });
  }

  private ticks = 0;

  /** Mueve un poco los recursos: la sparkline avanza una muestra. */
  private tickResources(): void {
    const next = { ...this.desk, machines: { ...this.desk.machines } };
    for (const id of ["a", "b"] as Machine[]) {
      const m = next.machines[id];
      if (!m?.resources) continue;
      const h = m.resources.history;
      const last = h[h.length - 1] ?? 30;
      const drift = ((h.length * 7 + last) % 11) - 5;
      const v = Math.max(3, Math.min(97, last + drift));
      next.machines[id] = { ...m, resources: { ...m.resources, history: [...h.slice(1), v], cpu: { percent: v, label: `${v} %` } } };
    }
    this.set(next);
  }

  private set(d: DeskView): void {
    this.desk = d;
    this.emit(d);
  }

  async applyScene(id: string): Promise<ActResult> {
    const scene = this.desk.scenes.find((s) => s.id === id);
    if (!scene) return { ok: false, reason: "No existe esa escena" };
    this.set({ ...this.desk, activeScene: id });
    return { ok: true, text: `Escena «${scene.name}» aplicada` };
  }

  async setPower(machine: Machine, on: boolean): Promise<ActResult> {
    const m = this.desk.machines[machine];
    if (!m) return { ok: false, reason: "No se conoce esa máquina" };
    if (m.here) return { ok: false, reason: "Esta máquina no se apaga desde aquí" };
    const spec = demoDesk(this.opts).machines[machine];
    this.set({
      ...this.desk,
      machines: { ...this.desk.machines, [machine]: { ...m, power: on ? "on" : "off", resources: on ? (spec?.resources ?? null) : null } },
    });
    return { ok: true, text: on ? `${m.name} encendida` : `${m.name} apagada` };
  }

  /** Mide al instante lo que da cada camino en el demo. */
  async measureLink(path?: LinkKind): Promise<ActResult> {
    const kind = path ?? this.desk.link.kind;
    const target = this.desk.network.paths.find((p) => p.kind === kind);
    if (!kind || !target || (target.state !== "primary" && target.state !== "backup")) return { ok: false, reason: "Sin enlace con la otra máquina por ese camino" };
    const gbps = DEMO_GBPS[kind];
    const paths = this.desk.network.paths.map((p) => (p.kind === kind ? { ...p, gbps } : p));
    const link = this.desk.link.kind === kind ? { ...this.desk.link, gbps } : this.desk.link;
    this.set({ ...this.desk, link, network: { ...this.desk.network, paths, measuring: null } });
    return { ok: true, text: gbps >= 1 ? `${gbps} Gbps` : `${Math.round(gbps * 1000)} Mbps` };
  }

  async setModule(machine: Machine, module: string, on: boolean): Promise<ActResult> {
    const m = this.desk.machines[machine];
    const mod = m?.modules.find((x) => x.id === module);
    if (!m || !mod) return { ok: false, reason: "No existe ese módulo" };
    if (mod.state === "unavailable") return { ok: false, reason: mod.detail };
    const modules = m.modules.map((x) => (x.id === module ? { ...x, state: on ? ("active" as const) : ("off" as const) } : x));
    this.set({ ...this.desk, machines: { ...this.desk.machines, [machine]: { ...m, modules } } });
    return { ok: true };
  }

  private pairTimer: ReturnType<typeof setTimeout> | undefined;
  private keys: ShortcutsView = { bindings: { ...DEFAULT_BINDINGS }, failed: {}, warning: null, file: null };

  private setPairing(pairing: PairingView, peer = this.desk.pair.peer): void {
    this.set({ ...this.desk, pair: { peer, pairing } });
  }

  /** Simula a la otra máquina: aparece el código y, al confirmar, confirma ella también. */
  async setArrangement(a: Arrangement): Promise<ActResult> {
    if (this.desk.trouble === "no-agent") return { ok: false, reason: "El agente de Tandem no está corriendo en esta máquina" };
    this.set({ ...this.desk, layout: { ...this.desk.layout, arrangement: { ...a } } });
    return { ok: true, text: "Acomodo guardado" };
  }

  private crossers = new Set<(s: CrossSample) => void>();

  /** La prueba de cruce: con `cross=…` termina como se pidió; si no, cruza bien a los pocos segundos. */
  async watchCrossing(onCross: (s: CrossSample) => void): Promise<() => void> {
    this.crossers.add(onCross);
    const { here, layout } = this.desk;
    const edge = exitSide(layout.arrangement, here);
    const how = this.opts.cross;
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (how === "ok" || how === "wrong") {
      // El borde equivocado: uno de los dos que no son el del acomodo ni el opuesto.
      const wrong = isVertical(edge) ? "up" : "right";
      this.cross({ from: here, to: here === "a" ? "b" : "a", edge: how === "ok" ? edge : wrong, at: 612, of: 1440, ms: 0.4 });
    } else if (!how && !this.opts.still) {
      timer = setTimeout(() => this.cross({ from: here, to: here === "a" ? "b" : "a", edge, at: 612, of: 1440, ms: 0.4 }), CROSS_MS);
    }
    return () => {
      clearTimeout(timer);
      this.crossers.delete(onCross);
    };
  }

  /** Un cruce del cursor (lo que informaría el motor de entrada). */
  cross(s: CrossSample): void {
    for (const f of [...this.crossers]) queueMicrotask(() => f(s));
  }

  /** Con `cross=timeout` la prueba se queda sin tiempo apenas empieza. */
  get crossTimeoutMs(): number | undefined {
    return this.opts.cross === "timeout" ? 0 : undefined;
  }

  async pairAction(kind: PairActionKind): Promise<ActResult> {
    clearTimeout(this.pairTimer);
    const peerName = this.desk.machines[this.desk.here === "a" ? "b" : "a"]?.name ?? "la otra máquina";
    const p = this.desk.pair.pairing;
    switch (kind) {
      case "start_pairing":
        this.setPairing({ kind: "searching" });
        this.pairTimer = setTimeout(() => this.setPairing({ kind: "comparing", code: DEMO_CODE, peer: peerName, confirmed: false }), PAIR_FOUND_MS);
        return { ok: true };
      case "confirm_pairing":
        if (p.kind !== "comparing") return { ok: false, reason: "No hay código que confirmar" };
        this.setPairing({ ...p, confirmed: true });
        this.pairTimer = setTimeout(() => {
          if (this.opts.pairing === "reject") this.setPairing({ kind: "failed", reason: `se rechazó en ${peerName}` });
          else this.setPairing({ kind: "done", peer: peerName }, { kind: "paired", name: peerName, link: "usb4" });
        }, PAIR_OTHER_MS);
        return { ok: true };
      case "cancel_pairing":
        this.setPairing({ kind: "idle" });
        return { ok: true };
      case "unpair":
        this.setPairing({ kind: "idle" }, { kind: "unpaired" });
        return { ok: true };
    }
  }

  async requestPermissions(): Promise<ActResult> {
    return { ok: true };
  }

  /** En el demo, abrir el panel equivale a que el usuario lo concede. */
  async openPermission(p: Permission): Promise<ActResult> {
    const perms = this.desk.permissions;
    const rows = perms.rows.map((r) => (r.id === p && r.state === "missing" ? { ...r, state: "granted" as const } : r));
    const trouble = rows.some((r) => r.state === "missing") ? this.desk.trouble : this.desk.trouble === "permissions" ? "none" : this.desk.trouble;
    this.set({ ...this.desk, trouble, troubleText: trouble === "none" ? "" : this.desk.troubleText, permissions: { ...perms, rows } });
    return { ok: true };
  }

  async restartAgent(): Promise<ActResult> {
    return { ok: true, text: "tandemd se reinicia: en unos segundos se vuelven a comprobar los permisos" };
  }

  private setupShown = false;

  async setup(): Promise<SetupView> {
    const needed = this.desk.permissions.needed;
    return { available: needed, show: needed && !!this.opts.firstRun && !this.setupShown, permissions: needed ? ["accessibility", "input_monitoring", "local_network"] : [], agent: null };
  }

  async finishSetup(): Promise<void> {
    this.setupShown = true;
  }

  async shortcuts(): Promise<ShortcutsView> {
    return this.keys;
  }

  async setShortcuts(b: Bindings): Promise<ShortcutsView> {
    this.keys = { ...this.keys, bindings: { ...b }, failed: {} };
    return this.keys;
  }

  async hideMain(): Promise<void> {}

  // Actualizaciones simuladas (`F-Ajustes`): las dos en 0.3.0 y, al buscar, aparece 0.3.1.
  private upd: UpdateView = { here: "0.3.0", peer: "0.3.0", paired: true, available: null, checking: false, step: null, error: null, auto: true };
  private onUpd: ((v: UpdateView) => void) | null = null;

  private setUpd(v: UpdateView): UpdateView {
    this.upd = v;
    this.onUpd?.(v);
    return v;
  }

  async updates(onChange: (v: UpdateView) => void): Promise<UpdateView> {
    this.onUpd = onChange;
    return this.upd;
  }

  async checkUpdates(): Promise<UpdateView> {
    return this.setUpd({ ...this.upd, checking: false, available: "0.3.1", error: null });
  }

  /** En el demo no se instala nada: se muestra el primer paso y queda ahí. */
  async updateAll(): Promise<UpdateView> {
    if (!this.upd.available) throw new Error("no hay una versión nueva: busca primero");
    return this.setUpd({ ...this.upd, step: "ask_peer" });
  }

  async setAutoUpdate(on: boolean): Promise<UpdateView> {
    return this.setUpd({ ...this.upd, auto: on });
  }

  can(): boolean {
    return true;
  }

  stop(): void {
    clearInterval(this.timer);
    clearTimeout(this.pairTimer);
    this.timer = undefined;
  }

  get currentTopics(): Topic[] {
    return this.topics;
  }
}
