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
// - `txn=run|ok|fail`: la pantalla de progreso de una escena ya en curso, terminada o
//   deshaciéndose (para capturas). `fail=<escena>`: aplicar esa escena falla y se deshace.
//   Con `still`, un cambio de entrada o de escena se aplica al instante y no agrega eventos ni
//   cambia la escena activa: así las capturas no dependen del reloj.

import { exitSide, isVertical, type Arrangement, type LayoutScreen, type LayoutView } from "../acomodo";
import type { CrossSample } from "../crossing";
import type { AdminView, Bindings, MonitorInput, PairingView, Permission, SceneJson, SetupView, ShortcutsView, Step, TxnView } from "../model";
import { measuredText } from "../network";
import { sceneView } from "../scenes";
import { DEFAULT_BINDINGS } from "../shortcuts";
import { checkName } from "../machine";
import { reducedMotion } from "../motion";
import { DEMO_LOOP_MS, demoPath, sampleAt, sceneOf } from "../puesto";
import type { UpdateView } from "../updates";
import { EMPTY_NETWORK, INPUT_TARGET_MS } from "./types";
import type {
  ActResult,
  Capability,
  CursorEvent,
  DeskChange,
  TransferView,
  DeskView,
  FilesView,
  EnvView,
  HardwareView,
  LatencyPoint,
  LinkKind,
  LiveView,
  Machine,
  MachineView,
  ModuleView,
  MonitorUi,
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
  OutputChunk,
  OutputCursor,
  ProcessRow,
  TunnelState,
  LinkView,
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
  /** Abre el asistente en ese paso (0 a 5), para capturas y pruebas. */
  wizard?: number;
  /** macOS con Accesibilidad y Monitorización sin conceder y Red local concedida (`F-Asis3`). */
  perms?: "missing";
  /** El servicio de administrador de macOS sin instalar: el asistente lo muestra pendiente (`F-Asis1`). */
  admin?: "pending" | "off";
  /** Cómo termina la prueba de cruce. */
  cross?: "ok" | "wrong" | "timeout";
  /** La tercera escena se llama «Solo PC», como en `F-Ajustes` (el resto del mockup la llama «Juego»). */
  soloPc?: boolean;
  /** La pantalla de progreso de una escena en este punto. */
  txn?: "run" | "ok" | "fail";
  /** Aplicar esta escena falla en su monitor y se deshace. */
  failScene?: string;
  /** Los túneles del demo: los de `V3-Computo`, ninguno o uno con 12 procesos. */
  tunnels?: "normal" | "none" | "many" | "notas" | "paleta" | "bandeja";
  /** Los datos que dibuja `F-Paleta`: «Trabajo» sobre la Mac, la tercera escena «Solo PC» y un túnel por proyecto. */
  paleta?: boolean;
  /** Los datos que dibuja `V3-Bandeja`: enlace de 0.4 ms, carga de la Mac al 18 %, «Solo PC» y túneles en tres proyectos. */
  bandeja?: boolean;
  /** La otra máquina cambia el puesto poco después de abrir (aviso «Cambiado desde…», D5). */
  peerEdit?: boolean;
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
    wizard: p.has("wizard") && Number.isInteger(Number(p.get("wizard"))) ? Number(p.get("wizard")) : undefined,
    perms: p.get("perms") === "missing" ? "missing" : undefined,
    txn: p.get("txn") === "run" || p.get("txn") === "ok" || p.get("txn") === "fail" ? (p.get("txn") as "run" | "ok" | "fail") : undefined,
    failScene: p.get("fail") ?? undefined,
    tunnels: p.has("paleta") ? "paleta" : p.has("bandeja") ? "bandeja" : p.get("tunnels") === "none" ? "none" : p.get("tunnels") === "many" ? "many" : p.get("tunnels") === "notas" ? "notas" : "normal",
    paleta: p.has("paleta"),
    bandeja: p.has("bandeja"),
    peerEdit: p.has("peeredit"),
    admin: p.get("admin") === "pending" ? "pending" : p.get("admin") === "off" ? "off" : undefined,
    soloPc: p.has("solopc") || p.has("paleta") || p.has("bandeja"),
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
  system: string;
  hardware: HardwareView;
  environments: EnvView[];
  inputDevices?: { name: string; via?: string }[];
  live: LiveView;
}

const GIB = 2 ** 30;

/** Una línea de los mockups (alturas `y` en un lienzo de `h`) como valores 0–100. */
const fromY = (h: number, ys: number[]) => ys.map((y) => ((h - y) / h) * 100);

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
  system: "macOS 26",
  hardware: { cpu: "Apple M5", cores: 12, memory: 24 * GIB, gpu: { name: "Apple M5", vram: null }, fanless: true },
  environments: [
    { id: "zsh", kind: "shell", name: "zsh", default: true, ready: true, detail: "uv · node · pnpm · cargo · git" },
    { id: "docker", kind: "docker", name: "Docker", default: false, ready: true, detail: "arm64 · sin GPU" },
  ],
  inputDevices: [{ name: "Teclado integrado" }, { name: "Trackpad" }, { name: "Keychron K3 · MX Master 3S", via: "por el hub" }],
  live: {
    cpu: { percent: 18, temp: 48, history: fromY(34, [28, 26, 29, 24, 27, 25, 28, 26, 27]) },
    ram: { used: 12.4 * GIB, total: 24 * GIB, history: fromY(22, [14, 13, 13, 12, 12, 11, 12, 12, 11]) },
    gpu: { percent: 6, vramUsed: 1.2 * GIB, vramTotal: null, temp: 44, fans: null, history: fromY(26, [22, 21, 22, 19, 22, 21, 22, 21, 22]) },
    disk: { free: 231 * GIB, total: 1024 * GIB },
    battery: { percent: 84, plugged: true },
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
  screens: [G34, G32],
  system: "Windows 11",
  hardware: { cpu: "AMD Ryzen 7", cores: 16, memory: 24 * GIB, gpu: { name: "NVIDIA GeForce RTX 5080", vram: 16 * GIB } },
  environments: [
    { id: "pwsh", kind: "shell", name: "PowerShell 7", default: true, ready: true, detail: "winget · python · git" },
    { id: "wsl:Ubuntu-24.04", kind: "wsl", name: "Ubuntu 24.04", default: true, ready: true, detail: "uv · cargo · node · CUDA 12.8 · git" },
    { id: "docker", kind: "docker", name: "Docker Desktop", default: false, ready: true, detail: "GPU: --gpus all disponible" },
  ],
  inputDevices: [],
  live: {
    cpu: { percent: 64, temp: 62, history: fromY(34, [26, 24, 28, 20, 18, 15, 17, 14, 16]) },
    ram: { used: 17.1 * GIB, total: 24 * GIB, history: fromY(22, [14, 13, 13, 11, 10, 9, 9, 8, 8]) },
    gpu: { percent: 97, vramUsed: 14.1 * GIB, vramTotal: 16 * GIB, temp: 71, fans: 58, history: fromY(26, [8, 5, 6, 4, 5, 4, 5, 4, 4]) },
    disk: { free: 412 * GIB, total: 1.8 * 1024 * GIB },
    battery: null,
  },
};

const WORKSHOP: Spec = {
  ...DESKTOP,
  name: "PC-Taller",
  color: "#6cc59a",
  detail: "Windows · Ryzen 7",
  resources: { ...DESKTOP.resources, gpu: null, gpuLabel: "GPU", cpu: { percent: 31, label: "31 %" } },
  screens: [U27],
  system: "Windows 11",
  hardware: { ...DESKTOP.hardware, gpu: null },
  live: { ...DESKTOP.live, cpu: { ...DESKTOP.live.cpu, percent: 31 }, gpu: null },
};

const MINI: Spec = {
  ...MACBOOK,
  name: "Mac mini",
  color: "#d9a75a",
  detail: "macOS · M4",
  resources: { ...MACBOOK.resources, cpu: { percent: 9, label: "9 %" } },
  screens: [STUDIO],
  hardware: { ...MACBOOK.hardware, cpu: "Apple M4", gpu: { name: "Apple M4", vram: null } },
  live: { ...MACBOOK.live, cpu: { ...MACBOOK.live.cpu, percent: 9 } },
};

const PAIRS: Record<DemoPair, [Spec, Spec]> = {
  "mac-win": [MACBOOK, DESKTOP],
  "win-win": [DESKTOP, WORKSHOP],
  "mac-mac": [MACBOOK, MINI],
};

function modulesOf(os: Os, name: string, otherName: string, envs: number, adminPending = false): ModuleView[] {
  const mac = os === "macos";
  // Con un permiso de macOS faltando, la página lo muestra en su propia fila («Permisos», como
  // `F-MaquinaMac`); el módulo sigue con lo que sí puede hacer.
  const list: ModuleView[] = [
    { id: "input", name: "Teclado y mouse", state: "active", detail: mac ? "Captura con CGEventTap e inyección con CGEventPost" : "Captura e inyección (hooks de bajo nivel, SendInput)", switchable: true },
    { id: "clipboard", name: "Portapapeles", state: "active", detail: "Texto, HTML e imagen", switchable: true },
    mac
      ? { id: "monitors", name: "Monitores", state: "off", detail: `Por HDMI solo hay canal DDC mientras el monitor muestra la ${name.split(" ")[0]}. Hoy los controla ${otherName}.`, switchable: true }
      : { id: "monitors", name: "Monitores", state: "active", detail: 'Canal DDC por DisplayPort a 2 monitores. Dueño del 34".', switchable: true },
    {
      id: "power",
      name: "Energía",
      state: mac ? "off" : "active",
      detail: mac ? "Puede despertar a otras por Wake-on-LAN. No se apaga sola." : `Se apaga sola. Se despierta por Wake-on-LAN desde ${otherName}.`,
      switchable: false,
    },
    mac
      ? adminPending
        ? { id: "tunnels", name: "Cómputo remoto", state: "off", detail: `${envs} entornos · falta instalar el servicio de administrador`, switchable: true }
        : { id: "tunnels", name: "Cómputo remoto", state: "active", detail: `${envs} entornos · admin (LaunchDaemon instalado)`, switchable: true }
      : { id: "tunnels", name: "Túneles", state: "active", detail: `Recibe túneles de Claude en ${envs} entornos, como administrador.`, switchable: true },
  ];
  if (!mac) list.push({ id: "secure_desktop", name: "Pantallas protegidas", state: "active", detail: "El mouse y el teclado también funcionan en avisos de UAC y en el inicio de sesión.", switchable: true });
  return list;
}

const ctrlAlt = (n: number): string[] => ["ControlLeft", "AltLeft", `Digit${n}`];

/** Los monitores del demo: los externos de cada máquina. Los desconectados no son compatibles (D11). */
function demoMonitors(sa: Spec, sb: Spec): MonitorUi[] {
  const out: MonitorUi[] = [];
  for (const [owner, spec] of [["b", sb] as const, ["a", sa] as const]) {
    for (const d of spec.screens.filter((x) => x.external)) {
      if (d.off) {
        out.push({ id: d.id, name: d.name, compat: { ok: false, reason: "Entra solo cuando su firmware cumpla el estándar." }, connected: false, owner: null, input: null, ports: {}, shows: null, events: [] });
        continue;
      }
      const input: MonitorInput = owner === "a" ? "hdmi" : "display_port";
      // La hora del demo es fija (hora local): las capturas no dependen de la zona horaria.
      const at = (h: number, m: number) => new Date(2026, 10, 17, h, m).getTime();
      out.push({
        id: d.id,
        name: d.name,
        compat: { ok: true },
        connected: true,
        owner,
        input,
        ports: { a: "hdmi", b: "display_port" },
        shows: owner,
        events: [
          { at: at(14, 2), kind: "switch", text: "Entrada → DP, pedida por Tandem" },
          { at: at(13, 47), kind: "button", text: "Entrada → HDMI, con los botones" },
          { at: at(13, 47), kind: "signal", text: "Señal HDMI detectada" },
        ],
      });
    }
  }
  return out.sort((x, y) => Number(y.compat.ok) - Number(x.compat.ok));
}

/** Las escenas del demo sobre el primer monitor compatible (como el mockup). */
export function demoScenes(a: string, b: string, monitors: MonitorUi[], mac: boolean, soloPc = false, trabajoEnA = false): SceneView[] {
  const m = monitors.find((x) => x.compat.ok)?.id;
  const raw: SceneJson[] = [
    { id: "trabajo", name: "Trabajo", monitors: m ? [[m, trabajoEnA ? "a" : "b"]] : [], shortcut: ctrlAlt(1) },
    { id: "solo-a", name: `Solo ${a.split(" ")[0]}`, monitors: m ? [[m, "a"]] : [], shortcut: ctrlAlt(2) },
    { id: "juego", name: soloPc ? "Solo PC" : "Juego", monitors: m ? [[m, "b"]] : [], shortcut: ctrlAlt(3) },
  ];
  return raw.map((r) => sceneView(r, { monitors, name: (x) => (x === "a" ? a : b), mac }));
}

const MIN = 60_000;
/** Medio minuto de más: las horas se leen «34 m» aunque pase un instante entre armar el demo y dibujarlo. */
const PAD = 20_000;

interface DemoProc {
  cmd: string;
  /** Hace cuánto empezó (ms). */
  ago: number;
  pid: number;
  log: string;
}

const p = (cmd: string, min: number, pid: number, log: string): DemoProc => ({ cmd, ago: min * MIN + PAD, pid, log });

/** Los procesos que corren en cada túnel del demo (los de `V3-Computo`) y su salida. */
const DEMO_PROCS: Record<string, DemoProc[]> = {
  t97: [
    p("cargo test -p tandem-monitor", 2, 4127, "   Compiling tandem-monitor v0.3.0\n     Running unittests src/lib.rs\ntest probe_never_sends_vendor_codes ... ok\ntest hotplug_adds_monitor ... ok\ntest owner_failover_when_pc_sleeps ... ok\ntest verdict_failed_rolls_back_scene ... "),
    p("monitor-events serve --sim 2", 30, 3810, "listening on 127.0.0.1:8765\nGET /api/monitors 200\nSSE monitor-added lc34g55t-9c1e\nSSE input-changed lc34g55t-9c1e dp\nSSE switch-done lc34g55t-9c1e 1.8s\n"),
    p("cargo watch -x check -p tandem-monitor", 25, 3902, "[Running cargo check]\n    Finished dev in 0.9s\n[Finished running. Exit status: 0]\n"),
    p("tandemd --sim --monitors 2", 22, 3955, "monitores: 2 detectados (1 compatible)\ndueño de lc34g55t-9c1e: pc-sim\n"),
    p("pnpm --filter tandem-ui dev", 20, 3988, "VITE v6 listo en 412 ms\n➜ Local: http://localhost:1420/\n"),
    p("tail -f ~/.tandem/logs/tandemd.log", 19, 4001, "14:06:12 monitor lc34g55t-9c1e input=dp (firmware)\n14:06:14 escena «Trabajo» aplicada\n"),
  ],
  t101: [
    p("cargo test --workspace", 3, 5210, "     Running tests/two_processes.rs\ntest tunnel_survives_link_drop ... ok\ntest tunnel_many_processes ... ok\ntest pairing_with_real_quic ... "),
    p("tandemd --sim --peers 2", 15, 5102, "peer mac-sim conectado por loopback\ntúnel t-01 abierto · 0.1 ms\n3 procesos vivos en t-01\n"),
  ],
  pr: [],
  lora: [
    p("uv run train.py --config estilo.toml", 41, 2290, "epoch 7/20  step 1860/5200  loss 0.0398\nguardado out/estilo-e7.safetensors\nepoch 8/20  step 1880/5200  loss 0.0405\n"),
    p("python main.py --listen 127.0.0.1", 60, 2104, "Starting server on 127.0.0.1:8188\nGPU: RTX 5080 · 16 GB\nprompt executed in 4.2 s\n"),
    p("tensorboard --logdir out", 40, 2301, "TensorBoard 2.18 at http://localhost:6006/\n"),
  ],
  comfy: [],
};

/** Un túnel con muchos procesos (`tunnels=many`): la lista tiene que aguantarlos con scroll. */
const MANY_COMMANDS = [
  "cargo build --workspace",
  "cargo test -p tandem-link",
  "cargo clippy --workspace",
  "pnpm --filter tandem-ui dev",
  "pnpm --filter tandem-ui test --watch",
  "tandemd --sim --peers 2",
  "tail -f ~/.tandem/logs/tandemd.log",
  "monitor-events serve --sim 2",
  "uv run train.py --config estilo.toml",
  "python main.py --listen 127.0.0.1",
  "tensorboard --logdir out",
  "nvidia-smi -l 5",
];
const MANY: DemoProc[] = MANY_COMMANDS.map((c, i) => p(c, 3 + i * 4, 6000 + i, `$ ${c}\nlisto\n`));

/** El demo de los túneles: los cinco de `V3-Computo`, ninguno (`tunnels=none`) o uno con 12 procesos (`tunnels=many`). */
export function demoTunnels(now: number, which: DemoOptions["tunnels"] = "normal"): TunnelView[] {
  if (which === "none") return [];
  const t = (id: string, project: string, path: string, title: string, branch: string, from: Machine, to: Machine, state: TunnelState, openMin: number, env: string, o: Partial<TunnelView>): TunnelView => ({
    id,
    project,
    projectPath: path,
    title,
    from,
    to,
    state,
    processes: DEMO_PROCS[id]?.length ?? 0,
    branch,
    created: now - openMin * MIN - PAD,
    idleSince: null,
    closedAt: null,
    env,
    commands: 0,
    ports: 0,
    traffic: { up: 0, down: 0 },
    ...o,
  });
  const list = [
    t("t97", "tandem", "~/…/apps/tandem", "Crate de monitores (T-097)", "task/T-097", "a", "b", "busy", 34, "WSL · Ubuntu", { commands: 22, traffic: { up: 120e6, down: 4e6 } }),
    t("t101", "tandem", "~/…/apps/tandem", "Túneles en tandemd (T-101)", "task/T-101", "a", "b", "busy", 18, "WSL · Ubuntu", { commands: 12, traffic: { up: 64e6, down: 3e6 } }),
    t("pr", "tandem", "~/…/apps/tandem", "Revisar PR #66 en Windows", "develop", "b", "a", "idle", 41, "zsh", { commands: 9, ports: 1, idleSince: now - 9 * MIN - PAD, traffic: { up: 2e6, down: 18e6 } }),
    t("lora", "ia-local", "~/…/ia-local", "Entrenar LoRA de estilo", "main", "a", "b", "busy", 134, "WSL · Ubuntu", { commands: 37, ports: 1, traffic: { up: 2.1e9, down: 38e6 } }),
    t("comfy", "ia-local", "~/…/ia-local", "Probar flujo de ComfyUI", "main", "a", "b", "closed", 0, "Windows", { commands: 4, closedAt: now - 60 * MIN, traffic: { up: 12e6, down: 1e6 } }),
  ];
  if (which === "many") return [{ ...list[0], processes: MANY.length }, ...list.slice(1)];
  // `V3-Bandeja` dibuja la revisión del PR como un tercer proyecto («notas»); `V3-Computo` la deja en «tandem».
  // `F-Paleta` lista un túnel por proyecto, con el proyecto como título.
  if (which === "paleta") return [{ ...list[3], title: "ia-local" }, { ...list[0], title: "tandem" }];
  // `V3-Bandeja`: dos proyectos activos con un túnel cada uno y las «notas» en reposo.
  if (which === "bandeja") return [{ ...list[3], title: "ia-local" }, { ...list[0], title: "tandem" }, { ...list[2], project: "notas", projectPath: "~/…/notas" }];
  if (which === "notas") return list.map((x) => (x.id === "pr" ? { ...x, project: "notas", projectPath: "~/…/notas" } : x));
  return list;
}

/** Los procesos del demo de un túnel, como los daría el agente. */
export function demoProcesses(id: string, now: number, which: DemoOptions["tunnels"] = "normal"): ProcessRow[] {
  const procs = which === "many" && id === "t97" ? MANY : (DEMO_PROCS[id] ?? []);
  return procs.map((q, i) => ({ id: i + 1, command: q.cmd, pid: q.pid, startedAt: now - q.ago, state: "running", code: null }));
}

/** La salida del demo de un proceso (entera: no tiene cursor que avanzar). */
export function demoLog(id: string, process: number, which: DemoOptions["tunnels"] = "normal"): string {
  const procs = which === "many" && id === "t97" ? MANY : (DEMO_PROCS[id] ?? []);
  return procs[process - 1]?.log ?? "";
}

/** La hora del demo (fija: capturas deterministas). Los cambios de camino se fechan desde aquí. */
export const DEMO_NOW = 1_763_400_000_000;

/** Lo que da cada camino en el demo, en Gbps (los números de `V3-Red`). */
const DEMO_GBPS: Record<LinkKind, number> = { usb4: 9.4, ethernet: 2.3, wifi: 0.41 };

const HISTORY = 60;
/** Segundo del historial en que se desconectó el cable y en que volvió. */
const CUT_AT = 23;
const BACK_AT = 31;

const round = (n: number, d = 10) => Math.round(n * d) / d;

/**
 * El p99 de la entrada de cada segundo (ms): la línea baja de `V3-Red` y, entre el corte y la
 * vuelta del cable, el pico, que lleva el Wi-Fi.
 */
const DEMO_INPUT = [
  1.2, 1.22, 1.25, 1.27, 1.29, 1.27, 1.23, 1.18, 1.14, 1.11, 1.18, 1.25, 1.31, 1.38, 1.37, 1.32, 1.28, 1.23, 1.21, 1.23, 1.25, 1.27, 1.3, 2.15, 3.12,
  4.09, 5.3, 5.6, 3.9, 2.82, 2.05, 1.3, 1.28, 1.25, 1.23, 1.21, 1.21, 1.24, 1.26, 1.28, 1.3, 1.33, 1.35, 1.37, 1.39, 1.37, 1.32, 1.28, 1.23, 1.21,
  1.23, 1.25, 1.28, 1.3, 1.3, 1.3, 1.3, 1.3, 1.3, 1.3,
];

/**
 * El segundo `i` del historial de un camino (determinista). El USB-C lleva la entrada salvo
 * entre el corte y la vuelta, cuando la lleva el Wi-Fi: ahí está el pico de la gráfica.
 */
function demoPoint(kind: LinkKind, i: number): LatencyPoint {
  const jitter = ((i * 7) % 5) / 5;
  if (kind === "ethernet") return { rttMs: null, inputP99Ms: null };
  const cut = i >= CUT_AT && i < BACK_AT;
  if (kind === "usb4") {
    if (cut) return { rttMs: null, inputP99Ms: null };
    return { rttMs: round(0.3 + jitter * 0.1, 100), inputP99Ms: DEMO_INPUT[i % DEMO_INPUT.length] ?? null };
  }
  return { rttMs: round(3.6 + jitter * 0.6, 100), inputP99Ms: cut ? (DEMO_INPUT[i] ?? null) : null };
}

/** Lo que se sabe de cada enlace en el demo (los textos de `V3-Red`). */
const DEMO_DETAIL: Record<LinkKind, string> = { usb4: "USB4 · 20 Gbps", ethernet: "2.5 GbE", wifi: "Wi-Fi 7 · casa" };

/** La red del demo: USB-C principal, Ethernet sin cable y Wi-Fi de respaldo (`V3-Red`). */
export function demoNetwork(linkUp: boolean): NetworkView {
  if (!linkUp) return { ...EMPTY_NETWORK, paths: EMPTY_NETWORK.paths.map((p) => ({ ...p })) };
  const path = (kind: LinkKind, state: PathView["state"], rttMs: number | null, input: [number, number] | null, gbps: number | null): PathView => {
    const history = Array.from({ length: HISTORY }, (_, i) => demoPoint(kind, i));
    return { kind, state, rttMs, inputP50Ms: input?.[0] ?? null, inputP99Ms: input?.[1] ?? null, targetMs: INPUT_TARGET_MS[kind], gbps, history, detail: DEMO_DETAIL[kind] };
  };
  const at = (i: number) => DEMO_NOW - (HISTORY - 1 - i) * 1000;
  return {
    paths: [path("usb4", "primary", 0.3, [0.4, 1.9], DEMO_GBPS.usb4), path("ethernet", "unplugged", null, null, null), path("wifi", "backup", 3.8, null, DEMO_GBPS.wifi)],
    switches: [
      { at: at(BACK_AT), from: "wifi", to: "usb4", tookMs: null },
      { at: at(CUT_AT), from: "usb4", to: "wifi", tookMs: 600 },
    ],
    measuring: null,
    notices: [],
    now: DEMO_NOW,
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

function permissionsFor(os: Os, missing: boolean, netGranted = false): PermissionsView {
  if (os !== "macos") return { needed: false, rows: [], binary: null };
  return {
    needed: true,
    rows: [
      { id: "accessibility", state: missing ? "missing" : "granted" },
      { id: "input_monitoring", state: missing ? "missing" : "granted" },
      { id: "local_network", state: netGranted ? "granted" : "unknown" },
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
      const [pw, ph] = d.res.split("×").map((v) => parseInt(v, 10));
      const native = { id: d.id, x: 0, y: 0, w: pw ?? 0, h: ph ?? 0 };
      const id = sa.screens.includes(d) && sb.screens.includes(d) ? `${m}-${d.id}` : d.id;
      if (gone) {
        off.push({ ...rest, id, machine: null, x: 0, y: 0, usable: false, line: "Desconectado" });
        continue;
      }
      const shows = external && scene?.shows === m ? ` · escena ${scene.name}` : "";
      screens.push({ ...rest, id, machine: m, x: 0, y: 0, usable: true, line: `${spec.name}${shows}`, external: external === true, native });
    }
  }
  return { screens: [...screens, ...off], arrangement: { ...DEMO_ARRANGEMENT } };
}

/** Con el puesto emparejado la otra máquina va por el cable; antes solo se la encuentra por la red (Wi-Fi). */
const PAIRED_LINK: LinkView = { kind: "usb4", gbps: 9.4, ms: 0.3, inputP99: 1.9 };
/** `V3-Bandeja` dice 0.4 ms (el Puesto, 0.3). */
const TRAY_LINK: LinkView = { ...PAIRED_LINK, ms: 0.4 };
const UNPAIRED_LINK: LinkView = { kind: "wifi", gbps: 0.41, ms: 3.8, inputP99: null };
const NO_LINK: LinkView = { kind: null, gbps: null, ms: null, inputP99: null };

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
      resources: power === "on" ? { ...s.resources, ...(o.bandeja && s.os === "macos" ? { gpu: { percent: 18, label: "18 %" } } : {}), history: [...s.resources.history] } : null,
      modules: modulesOf(s.os, s.name, otherName, s.environments.length, o.admin === "pending"),
      system: s.system,
      hardware: s.hardware,
      environments: s.environments.map((e) => ({ ...e })),
      live: power === "on" ? s.live : null,
      ...(s.inputDevices ? { inputDevices: s.inputDevices } : {}),
    };
  };
  const monitors = demoMonitors(sa, sb).map((m) => (o.txn === "fail" && m.compat.ok ? { ...m, input: "hdmi" as const, shows: "a" as const, owner: "b" as const } : m));
  const scenes = demoScenes(sa.name, sb.name, monitors, (o.here === "a" ? sa : sb).os === "macos", o.soloPc, o.paleta);
  const linkUp = o.trouble !== "peer-off" && o.trouble !== "link-down" && o.trouble !== "no-agent";
  return {
    ready: true,
    deskName: "Mi puesto",
    prefs: { cmdCtrl: true, dragFiles: false, clipboard: true },
    lockShortcut: ["ScrollLock"],
    trouble: o.trouble,
    troubleText: TROUBLE_TEXT[o.trouble](peerSpec.name),
    here: o.here,
    machines: { a: mk("a", sa, sb.name), b: mk("b", sb, sa.name) },
    link: linkUp ? (o.unpaired ? UNPAIRED_LINK : o.bandeja ? TRAY_LINK : PAIRED_LINK) : NO_LINK,
    network: o.txn && linkUp ? { ...demoNetwork(linkUp), measuring: "usb4" } : demoNetwork(linkUp),
    tunnels: demoTunnels(o.still ? DEMO_NOW : Date.now(), o.tunnels),
    scenes,
    activeScene: "trabajo",
    monitors,
    pair: {
      peer: o.unpaired ? { kind: "unpaired" } : { kind: "paired", name: peerSpec.name, link: linkUp ? "usb4" : null },
      pairing: o.pairing === "code" ? { kind: "comparing", code: DEMO_CODE, peer: peerSpec.name, confirmed: false } : { kind: "idle" },
    },
    permissions: permissionsFor((o.here === "a" ? sa : sb).os, o.trouble === "permissions" || o.perms === "missing", o.perms === "missing"),
    layout: demoLayout(sa, sb, scenes, "trabajo"),
  };
}

/** Cuánto tarda el cruce simulado cuando nadie dijo cómo termina. */
const CROSS_MS = 2500;

const PAIR_FOUND_MS = 1400;
const PAIR_OTHER_MS = 1200;

/** Cuánto tarda la otra máquina en «editar» el puesto cuando el demo lo pide. */
const PEER_EDIT_MS = 1200;

const CURSOR_MS = 33;
const TICK_MS = 1500;
const RESCAN_MS = 900;
/** Lo que tarda la medición de ancho de banda en el demo. */
const DEMO_MEASURE_MS = 1500;

/** Lo que tarda cada paso de una transacción simulada. */
const STEP_MS = 700;

/** Una fuente simulada: responde a las acciones como lo haría el agente. */
export class DemoSource implements Source {
  readonly demo = true;
  private desk: DeskView;
  private emit: (d: DeskView) => void = () => {};
  private timer: ReturnType<typeof setInterval> | undefined;
  private topics: Topic[] = [];
  /** Lo que se pidió en vivo, para las pruebas. */
  readonly watched: Topic[][] = [];

  constructor(private readonly opts: DemoOptions) {
    this.desk = demoDesk(opts);
  }

  async start(onDesk: (d: DeskView) => void, onChange?: (c: DeskChange) => void, onTransfer?: (t: TransferView) => void): Promise<void> {
    this.emit = onDesk;
    this.onChange = onChange;
    this.onTransfer = onTransfer;
    onDesk(this.desk);
    if (this.opts.peerEdit) setTimeout(() => this.peerEdit(), PEER_EDIT_MS);
  }

  private onChange: ((c: DeskChange) => void) | undefined;
  private onTransfer: ((t: TransferView) => void) | undefined;

  /** El avance de una copia de archivos, como lo mandaría el agente. */
  simulateTransfer(t: TransferView): void {
    this.onTransfer?.(t);
  }

  /** La otra máquina cambió el puesto (D5): lo que haría `DeskChanged` del agente. */
  simulateChange(c: DeskChange): void {
    this.onChange?.(c);
  }

  /** Versión del puesto en el demo: cada cambio la sube. */
  rev = 1;
  /** El puesto tal como estaba en cada versión, para que «Deshacer» lo restaure de verdad. */
  private readonly history = new Map<number, DeskView>();

  /** Guarda el puesto actual como la versión vigente y sube la versión; devuelve la anterior. */
  private bump(): number {
    const before = this.rev;
    this.history.set(before, this.desk);
    this.rev += 1;
    return before;
  }

  /** Un cambio hecho desde la otra máquina (acomodo): se aplica y avisa con «Deshacer». */
  peerEdit(summary = "Teclado y mouse: el portapapeles compartido está apagado"): void {
    const undoTo = this.bump();
    this.set({ ...this.desk, prefs: { ...this.desk.prefs, clipboard: !this.desk.prefs.clipboard } });
    this.simulateChange({ by: this.desk.here === "a" ? "b" : "a", rev: this.rev, summary, undoTo });
  }

  async setLockShortcut(lock: string[]): Promise<ActResult> {
    this.set({ ...this.desk, lockShortcut: lock });
    return { ok: true, text: "Atajo de bloqueo guardado" };
  }

  async setPrefs(p: { cmdCtrl: boolean; dragFiles: boolean }): Promise<ActResult> {
    this.bump();
    this.set({ ...this.desk, prefs: { ...this.desk.prefs, ...p } });
    return { ok: true };
  }

  async undoDesk(rev: number): Promise<ActResult> {
    const before = this.history.get(rev);
    if (!before) return { ok: false, reason: "Esa versión del puesto ya no está" };
    this.bump();
    this.set(before);
    return { ok: true, text: "Cambio deshecho" };
  }

  async files(): Promise<FilesView> {
    const mac = this.desk.machines[this.desk.here]?.os !== "windows";
    return mac
      ? { config: "~/Library/Application Support/Tandem/tandem.toml", logs: "~/Library/Application Support/Tandem/logs" }
      : { config: "%APPDATA%\\Tandem\\tandem.toml", logs: "%APPDATA%\\Tandem\\logs" };
  }

  async openFiles(): Promise<ActResult> {
    return { ok: true };
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
    this.set({ ...this.desk, network: { ...this.desk.network, paths, now: DEMO_NOW + this.ticks * 1000 } });
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
      const live = m.live ? { ...m.live, cpu: { ...m.live.cpu, percent: v, history: [...m.live.cpu.history.slice(1), v] } } : null;
      next.machines[id] = { ...m, live, resources: { ...m.resources, history: [...h.slice(1), v], cpu: { percent: v, label: `${v} %` } } };
    }
    this.set(next);
  }

  private set(d: DeskView): void {
    this.desk = d;
    this.emit(d);
  }

  // ---- Monitores, escenas y su progreso --------------------------------------------------

  private txnListeners = new Set<(e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void>();
  private nextTxn = 1;
  private txnTimers: ReturnType<typeof setTimeout>[] = [];

  /** La hora de la fuente: fija en las capturas (progreso fijo o demo quieto), si no la del reloj. */
  now(): number {
    const t = this.opts.txn;
    return t ? DEMO_NOW + (t === "run" ? 1200 : t === "ok" ? 2300 : 3000) : this.opts.still ? DEMO_NOW : Date.now();
  }

  async watchTxn(onEvent: (e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void): Promise<() => void> {
    this.txnListeners.add(onEvent);
    if (this.opts.txn) this.replay(onEvent, this.opts.txn);
    return () => {
      this.txnListeners.delete(onEvent);
    };
  }

  /** El progreso de «Juego» en un punto fijo, con las horas de `F-Transacciones`. */
  private replay(emit: (e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void, at: "run" | "ok" | "fail"): void {
    const g34 = this.desk.monitors.find((m) => m.compat.ok);
    const steps: Step[] = [
      { step: "show_machine", monitor: g34?.id ?? "g34", machine: "b" },
      { step: "geometry" },
    ];
    const t0 = DEMO_NOW;
    const txn = 1;
    // Las otras dos tarjetas de `F-Transacciones`: la PC encendiéndose lleva 18 s esperando
    // (el reloj de la fuente corre a la par) y el enlace USB-C se está midiendo (en `demoDesk`).
    const boot = this.now() - 18_000;
    emit({ type: "started", txn: 2, steps: [{ step: "power_on", machine: "b" }] }, boot);
    emit({ type: "step_started", txn: 2, index: 0 }, boot + 100);
    emit({ type: "started", txn, steps }, t0, { title: "Aplicando la escena «Juego»", doneText: "Escena «Juego» activa." });
    emit({ type: "step_started", txn, index: 0 }, t0 + 100);
    if (at === "run") return;
    if (at === "fail") {
      emit({ type: "step_failed", txn, index: 0, error: "el monitor no mostró imagen en DisplayPort; sigue en HDMI" }, t0 + 3000);
      return;
    }
    emit({ type: "step_done", txn, index: 0 }, t0 + 1900);
    emit({ type: "step_started", txn, index: 1 }, t0 + 1900);
    emit({ type: "step_done", txn, index: 1 }, t0 + 2300);
    emit({ type: "committed", txn, state: { on: { a: true, b: true }, scene: "juego" } }, t0 + 2300);
  }

  private emitTxn(e: TxnView): void {
    for (const l of this.txnListeners) l(e);
  }

  /**
   * Corre una transacción simulada: cada paso tarda un poco y avisa. Con `still` todo ocurre
   * al instante. `failAt` hace fallar ese paso y deshace los anteriores.
   */
  private runTxn(steps: Step[], effect: (step: Step) => void, done: () => void, failAt: number | null = null): void {
    const txn = this.nextTxn++;
    const gap = this.opts.still ? 0 : STEP_MS;
    const after = (ms: number, f: () => void) => (ms === 0 ? f() : void this.txnTimers.push(setTimeout(f, ms)));
    this.emitTxn({ type: "started", txn, steps });
    const state = () => ({ on: { a: true, b: true }, scene: this.desk.activeScene });
    const undo = (from: number) => {
      for (let i = from; i >= 0; i--) after(gap * (from - i + 1), () => this.emitTxn({ type: "step_undone", txn, index: i }));
      after(gap * (from + 2), () => this.emitTxn({ type: "failed", txn, state: state() }));
    };
    const run = (i: number) => {
      if (i >= steps.length) {
        done();
        this.emitTxn({ type: "committed", txn, state: state() });
        return;
      }
      this.emitTxn({ type: "step_started", txn, index: i });
      after(gap, () => {
        if (i === failAt) {
          this.emitTxn({ type: "step_failed", txn, index: i, error: "el monitor no mostró imagen en DisplayPort; sigue en HDMI" });
          undo(i - 1);
          return;
        }
        effect(steps[i] as Step);
        this.emitTxn({ type: "step_done", txn, index: i });
        run(i + 1);
      });
    };
    run(0);
  }

  async switchMonitor(id: string, input: MonitorInput): Promise<ActResult> {
    const m = this.desk.monitors.find((x) => x.id === id);
    if (!m) return { ok: false, reason: "No se conoce ese monitor" };
    if (!m.compat.ok) return { ok: false, reason: `${m.name} no es compatible: no se le escribe nada` };
    if (!m.connected) return { ok: false, reason: `${m.name} está desconectado` };
    const to = (["a", "b"] as Machine[]).find((x) => m.ports[x] === input) ?? null;
    this.runTxn(
      [{ step: "switch_monitor", monitor: id, input }],
      () => this.setMonitor(id, input, to),
      () => {},
    );
    return { ok: true };
  }

  /** La entrada real de un monitor cambió (por el cambio pedido o por la escena). */
  private setMonitor(id: string, input: MonitorInput, shows: Machine | null): void {
    const monitors = this.desk.monitors.map((m) => {
      if (m.id !== id) return m;
      // Con `still` no se agrega el evento: llevaría la hora del reloj (capturas deterministas).
      const events = this.opts.still ? m.events : [{ at: Date.now(), kind: "switch" as const, text: `Entrada → ${input === "hdmi" ? "HDMI" : "DP"}, pedida por Tandem` }, ...m.events].slice(0, 3);
      return { ...m, input, shows, events };
    });
    this.set({ ...this.desk, monitors });
  }

  async applyScene(id: string): Promise<ActResult> {
    const scene = this.desk.scenes.find((s) => s.id === id);
    if (!scene) return { ok: false, reason: "No existe esa escena" };
    const steps: Step[] = [
      ...scene.monitors.map((x): Step => ({ step: "show_machine", monitor: x.monitor, machine: x.machine })),
      ...(scene.monitors.length > 0 ? [{ step: "geometry" } as Step] : []),
    ];
    const bad = this.opts.failScene === id && steps.length > 0 ? 0 : null;
    this.runTxn(
      steps,
      (st) => {
        if (st.step !== "show_machine") return;
        const m = this.desk.monitors.find((x) => x.id === st.monitor);
        const port = m?.ports[st.machine];
        if (port) this.setMonitor(st.monitor, port, st.machine);
      },
      () => this.set({ ...this.desk, activeScene: id }),
      bad,
    );
    return { ok: true };
  }

  async saveScene(scene: SceneJson, isNew: boolean): Promise<ActResult> {
    const ctx = { monitors: this.desk.monitors, name: (m: Machine) => this.desk.machines[m]?.name ?? m, mac: this.desk.machines[this.desk.here]?.os === "macos" };
    const id = scene.id || this.freeId(scene.name);
    const view = sceneView({ ...scene, id }, ctx);
    const exists = this.desk.scenes.some((s) => s.id === id);
    if (isNew && exists) return { ok: false, reason: `Ya hay una escena con el id «${id}»` };
    if (!isNew && !exists) return { ok: false, reason: "Esa escena ya no existe" };
    const scenes = isNew ? [...this.desk.scenes, view] : this.desk.scenes.map((s) => (s.id === id ? view : s));
    this.set({ ...this.desk, scenes });
    return { ok: true, text: `Escena «${scene.name}» guardada` };
  }

  private freeId(name: string): string {
    const base = name.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "escena";
    let id = base;
    for (let n = 2; this.desk.scenes.some((s) => s.id === id); n++) id = `${base}-${n}`;
    return id;
  }

  async forgetMonitor(id: string): Promise<ActResult> {
    const m = this.desk.monitors.find((x) => x.id === id);
    if (!m) return { ok: false, reason: "No se conoce ese monitor" };
    if (m.compat.ok || m.connected) return { ok: false, reason: `${m.name} está en uso: solo se olvidan los que no son compatibles y no están` };
    this.set({ ...this.desk, monitors: this.desk.monitors.filter((x) => x.id !== id) });
    return { ok: true, text: `${m.name} olvidado` };
  }

  async setPower(machine: Machine, on: boolean): Promise<ActResult> {
    const m = this.desk.machines[machine];
    if (!m) return { ok: false, reason: "No se conoce esa máquina" };
    if (m.here) return { ok: false, reason: "Esta máquina no se apaga desde aquí" };
    const spec = demoDesk(this.opts).machines[machine];
    this.set({
      ...this.desk,
      machines: { ...this.desk.machines, [machine]: { ...m, power: on ? "on" : "off", resources: on ? (spec?.resources ?? null) : null, live: on ? (spec?.live ?? null) : null } },
    });
    return { ok: true, text: on ? `${m.name} encendida` : `${m.name} apagada` };
  }

  /** Mide al instante lo que da cada camino en el demo. */
  async measureLink(path?: LinkKind): Promise<ActResult> {
    const kind = path ?? this.desk.link.kind;
    const target = this.desk.network.paths.find((p) => p.kind === kind);
    if (!kind || !target || (target.state !== "primary" && target.state !== "backup")) return { ok: false, reason: "Sin enlace con la otra máquina por ese camino" };
    if (!this.opts.still) {
      // La prueba real dura 3 s: la pantalla muestra qué camino se está midiendo.
      this.set({ ...this.desk, network: { ...this.desk.network, measuring: kind } });
      await new Promise((done) => setTimeout(done, DEMO_MEASURE_MS));
    }
    const gbps = DEMO_GBPS[kind];
    const paths = this.desk.network.paths.map((p) => (p.kind === kind ? { ...p, gbps } : p));
    const link = this.desk.link.kind === kind ? { ...this.desk.link, gbps } : this.desk.link;
    this.set({ ...this.desk, link, network: { ...this.desk.network, paths, measuring: null } });
    return { ok: true, text: measuredText(kind, gbps) };
  }

  async setModule(machine: Machine, module: string, on: boolean): Promise<ActResult> {
    const m = this.desk.machines[machine];
    const mod = m?.modules.find((x) => x.id === module);
    if (!m || !mod) return { ok: false, reason: "No existe ese módulo" };
    if (mod.state === "unavailable") return { ok: false, reason: mod.detail };
    const modules = m.modules.map((x) => (x.id === module ? { ...x, state: on ? ("active" as const) : ("off" as const) } : x));
    const prefs = module === "clipboard" ? { ...this.desk.prefs, clipboard: on } : this.desk.prefs;
    this.set({ ...this.desk, prefs, machines: { ...this.desk.machines, [machine]: { ...m, modules } } });
    return { ok: true };
  }

  /** Re-escanear tarda un poco, como en la máquina real; lo encontrado vuelve con el puesto. */
  async rescan(machine: Machine): Promise<ActResult> {
    const m = this.desk.machines[machine];
    if (!m) return { ok: false, reason: "No se conoce esa máquina" };
    if (m.power !== "on") return { ok: false, reason: `${m.name} está apagada` };
    await new Promise((r) => setTimeout(r, RESCAN_MS));
    const now = this.desk.machines[machine];
    if (now) this.set({ ...this.desk, machines: { ...this.desk.machines, [machine]: { ...now, environments: now.environments.map((e) => ({ ...e })) } } });
    return { ok: true };
  }

  /** Lo mismo que valida el puesto: nombre no vacío y corto, color de la paleta y distinto. */
  async editMachine(machine: Machine, edit: { name?: string; color?: string }): Promise<ActResult> {
    const m = this.desk.machines[machine];
    if (!m) return { ok: false, reason: `la máquina ${machine.toUpperCase()} todavía no se conoce` };
    let { name, color } = m;
    if (edit.name !== undefined) {
      const c = checkName(edit.name);
      if (!c.ok) return { ok: false, reason: c.reason };
      name = c.name;
    }
    if (edit.color !== undefined) {
      if (!/^#[0-9a-f]{6}$/i.test(edit.color)) return { ok: false, reason: `«${edit.color}» no es un color #rrggbb` };
      const other = this.desk.machines[machine === "a" ? "b" : "a"];
      if (other && other.color.toLowerCase() === edit.color.toLowerCase()) return { ok: false, reason: `ese color ya es el de ${other.name}` };
      color = edit.color;
    }
    this.set({ ...this.desk, machines: { ...this.desk.machines, [machine]: { ...m, name, color } } });
    return { ok: true };
  }

  private pairTimer: ReturnType<typeof setTimeout> | undefined;
  private keys: ShortcutsView = { bindings: { ...DEFAULT_BINDINGS }, failed: {}, warning: null, file: null };

  private setPairing(pairing: PairingView, peer = this.desk.pair.peer): void {
    const link = this.desk.link.kind === null ? this.desk.link : peer.kind === "paired" ? PAIRED_LINK : UNPAIRED_LINK;
    this.set({ ...this.desk, link, pair: { peer, pairing } });
  }

  /** Simula a la otra máquina: aparece el código y, al confirmar, confirma ella también. */
  async setArrangement(a: Arrangement): Promise<ActResult> {
    if (this.desk.trouble === "no-agent") return { ok: false, reason: "El agente de Tandem no está corriendo en esta máquina" };
    this.set({ ...this.desk, layout: { ...this.desk.layout, arrangement: { ...a } } });
    return { ok: true, text: "Acomodo guardado" };
  }

  private listeners = new Set<(m: Machine) => void>();

  /** Escuchar teclas en cada máquina; el demo las recibe de `heardKey` (las pruebas) o de la ventana. */
  async watchKeyProbe(onHeard: (m: Machine) => void): Promise<() => void> {
    this.listeners.add(onHeard);
    return () => void this.listeners.delete(onHeard);
  }

  /** Una tecla oída en `m`. */
  heardKey(m: Machine): void {
    for (const f of [...this.listeners]) f(m);
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

  /**
   * El cursor en vivo: sigue el recorrido del mockup (7 s, de la MacBook al 34" y de vuelta) y
   * avisa cada cruce. Con `still` (o sin movimiento permitido) manda solo el punto de partida.
   */
  async watchCursor(onEvent: (e: CursorEvent) => void): Promise<() => void> {
    const names = (m: Machine) => this.desk.machines[m]?.name ?? m;
    const colors = (m: Machine) => this.desk.machines[m]?.color ?? "";
    let last: Machine | null = null;
    const step = (t: number) => {
      const scene = sceneOf(this.desk.layout, { a: names("a"), b: names("b") }, { a: colors("a"), b: colors("b") });
      const sample = sampleAt(scene, this.desk.layout, demoPath(t));
      if (!sample) return;
      if (last && last !== sample.machine) onEvent({ kind: "crossed", from: last, to: sample.machine, edge: exitSide(this.desk.layout.arrangement, last) });
      last = sample.machine;
      onEvent({ kind: "cursor", sample });
    };
    const first = setTimeout(() => step(0), 0);
    let timer: ReturnType<typeof setInterval> | undefined;
    if (!this.opts.still && !reducedMotion()) {
      const t0 = Date.now();
      timer = setInterval(() => step(((Date.now() - t0) % DEMO_LOOP_MS) / DEMO_LOOP_MS), CURSOR_MS);
    }
    return () => {
      clearTimeout(first);
      clearInterval(timer);
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

  /** El demo no tiene motor de entrada: el bloqueo se acepta sin más. */
  async lockHere(_on: boolean): Promise<ActResult> {
    return { ok: true };
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

  private adminDone = false;

  async adminStatus(): Promise<AdminView> {
    const off = this.opts.admin === "off";
    const state: AdminView["state"] = this.adminDone ? "ok" : (this.opts.admin ?? "ok");
    return { state, canInstall: state !== "ok" && !off, note: off ? "instalación por usuario: instala Tandem en Archivos de programa para el servicio" : "" };
  }

  async installAdmin(): Promise<AdminView> {
    if (this.opts.admin === "off") throw new Error("instalación por usuario: no aplica");
    this.adminDone = true;
    return this.adminStatus();
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

  async processes(tunnel: string): Promise<ProcessRow[]> {
    return demoProcesses(tunnel, this.now(), this.opts.tunnels);
  }

  async output(tunnel: string, process: number, from: OutputCursor): Promise<OutputChunk> {
    const text = demoLog(tunnel, process, this.opts.tunnels);
    // El demo da toda la salida de una vez; con el cursor al final no hay nada nuevo.
    return { text: from.stdout > 0 ? "" : text, next: { stdout: text.length, stderr: 0 }, done: false };
  }

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

  /** Todo se puede simular; medir, solo con un camino que responda. */
  can(what: Capability): boolean {
    return what !== "measure" || this.desk.link.kind !== null;
  }

  stop(): void {
    clearInterval(this.timer);
    clearTimeout(this.pairTimer);
    for (const t of this.txnTimers) clearTimeout(t);
    this.txnTimers = [];
    this.timer = undefined;
  }

  get currentTopics(): Topic[] {
    return this.topics;
  }
}
