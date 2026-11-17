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

import type { ActResult, DeskView, Machine, MachineView, ModuleView, Os, Resources, SceneView, Source, Topic, Trouble, TunnelView } from "./types";

export type DemoPair = "mac-win" | "win-win" | "mac-mac";

export interface DemoOptions {
  pair: DemoPair;
  here: Machine;
  trouble: Trouble;
  still: boolean;
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
  };
}

export function isDemo(query: string): boolean {
  return new URLSearchParams(query).has("demo");
}

const BLUE = "#7aa7e6";
const VIOLET = "#a99be8";

interface Spec {
  name: string;
  os: Os;
  color: string;
  detail: string;
  resources: Resources;
}

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
};

const WORKSHOP: Spec = {
  ...DESKTOP,
  name: "PC-Taller",
  color: "#6cc59a",
  detail: "Windows · Ryzen 7",
  resources: { ...DESKTOP.resources, gpu: null, gpuLabel: "GPU", cpu: { percent: 31, label: "31 %" } },
};

const MINI: Spec = {
  ...MACBOOK,
  name: "Mac mini",
  color: "#d9a75a",
  detail: "macOS · M4",
  resources: { ...MACBOOK.resources, cpu: { percent: 9, label: "9 %" } },
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

const TROUBLE_TEXT: Record<Trouble, (peer: string) => string> = {
  none: () => "",
  "no-agent": () => "El agente de Tandem no está corriendo en esta máquina",
  "peer-off": (peer) => `${peer} está apagada`,
  "link-down": (peer) => `Sin enlace con ${peer}`,
  permissions: () => "Faltan permisos en esta máquina",
};

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
  const linkUp = o.trouble !== "peer-off" && o.trouble !== "link-down" && o.trouble !== "no-agent";
  return {
    ready: true,
    trouble: o.trouble,
    troubleText: TROUBLE_TEXT[o.trouble](peerSpec.name),
    here: o.here,
    machines: { a: mk("a", sa, sb.name), b: mk("b", sb, sa.name) },
    link: linkUp ? { kind: "usb4", gbps: 9.4, ms: 0.3, inputP99: 1.9 } : { kind: null, gbps: null, ms: null, inputP99: null },
    tunnels: DEMO_TUNNELS.map((t) => ({ ...t })),
    scenes: DEMO_SCENES(sa.name, sb.name),
    activeScene: "trabajo",
  };
}

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
    // Solo los recursos cambian solos, y solo si la pantalla visible los muestra.
    if (!this.opts.still && topics.includes("resources")) this.timer = setInterval(() => this.tick(), TICK_MS);
  }

  /** Mueve un poco los recursos: la sparkline avanza una muestra. */
  tick(): void {
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

  async measureLink(): Promise<ActResult> {
    if (!this.desk.link.kind) return { ok: false, reason: "Sin enlace con la otra máquina" };
    this.set({ ...this.desk, link: { ...this.desk.link, gbps: 9.4 } });
    return { ok: true, text: "9.4 Gbps" };
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

  can(): boolean {
    return true;
  }

  stop(): void {
    clearInterval(this.timer);
    this.timer = undefined;
  }

  get currentTopics(): Topic[] {
    return this.topics;
  }
}
