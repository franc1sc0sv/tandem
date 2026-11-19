// Demo mode: a simulated, coherent desk for each screen and each state. Used by the
// screenshots (`pnpm shots`), the tests and `pnpm dev` (`/?demo`). The data is that of the sample
// mockups; in the real app everything comes from each machine's scan (D10).
//
// URL parameters:
// - `demo` or `demo=mac-win` (the default), `demo=win-win`, `demo=mac-mac`: which systems are
//   on each side (D2).
// - `here=b`: the window is machine B's.
// - `trouble=no-agent|peer-off|link-down|permissions`: a trouble state (`V3-States`).
// - `still`: no resources that change by themselves (for deterministic screenshots).
// - `peer=none`: unpaired; pairing simulates the other machine (code and confirmation).
//   `pair=code` starts by showing the code; `pair=reject`: the other machine rejects.
// - `cross=ok|wrong|timeout`: the crossing test crosses through the arrangement edge, through another one,
//   or times out right away (for screenshots). Without this (and without `still`), it crosses
//   fine after a few seconds.
// - `txn=run|ok|fail`: the progress screen of a scene in progress, finished or
//   being undone (for screenshots). `fail=<scene>`: applying that scene fails and is undone.
//   With `still`, an input or scene change is applied instantly and adds no events or
//   changes the active scene: so screenshots do not depend on the clock.

import { exitSide, isVertical, type Arrangement, type LayoutScreen, type LayoutView } from "../arrangement";
import type { CrossSample } from "../crossing";
import type { AdminView, Bindings, MonitorInput, PairingView, Permission, SceneJson, SetupView, ShortcutsView, Step, TxnView } from "../model";
import { measuredText } from "../network";
import { sceneView } from "../scenes";
import { DEFAULT_BINDINGS } from "../shortcuts";
import { checkName } from "../machine";
import { reducedMotion } from "../motion";
import { DEMO_LOOP_MS, demoPath, sampleAt, sceneOf } from "../desk";
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
  /** Starts unpaired. */
  unpaired?: boolean;
  /** How the other machine responds to pairing. */
  pairing?: "code" | "reject";
  /** The first-run permissions guide opens by itself. */
  firstRun?: boolean;
  /** Opens the setup at that step (0 to 5), for screenshots and tests. */
  wizard?: number;
  /** macOS with Accessibility and Monitoring not granted and Local network granted (`F-Setup3`). */
  perms?: "missing";
  /** The macOS administrator service not installed: the setup shows it pending (`F-Setup1`). */
  admin?: "pending" | "off";
  /** How the crossing test ends. */
  cross?: "ok" | "wrong" | "timeout";
  /** The third scene is called "PC only", as in `F-Settings` (the rest of the mockup calls it "Game"). */
  pcOnly?: boolean;
  /** The progress screen of a scene at this point. */
  txn?: "run" | "ok" | "fail";
  /** Applying this scene fails on its monitor and is undone. */
  failScene?: string;
  /** The demo tunnels: those of `V3-Compute`, none or one with 12 processes. */
  tunnels?: "normal" | "none" | "many" | "notes" | "palette" | "tray";
  /** The data `F-Palette` draws: "Work" on the Mac, the third scene "PC only" and one tunnel per project. */
  palette?: boolean;
  /** The data `V3-Tray` draws: 0.4 ms link, Mac load at 18 %, "PC only" and tunnels in three projects. */
  tray?: boolean;
  /** The other machine changes the desk shortly after opening ("Changed from…" notice, D5). */
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
    tunnels: p.has("palette") ? "palette" : p.has("tray") ? "tray" : p.get("tunnels") === "none" ? "none" : p.get("tunnels") === "many" ? "many" : p.get("tunnels") === "notes" ? "notes" : "normal",
    palette: p.has("palette"),
    tray: p.has("tray"),
    peerEdit: p.has("peeredit"),
    admin: p.get("admin") === "pending" ? "pending" : p.get("admin") === "off" ? "off" : undefined,
    pcOnly: p.has("pconly") || p.has("palette") || p.has("tray"),
    cross: p.get("cross") === "ok" ? "ok" : p.get("cross") === "wrong" ? "wrong" : p.get("cross") === "timeout" ? "timeout" : undefined,
  };
}

export function isDemo(query: string): boolean {
  return new URLSearchParams(query).has("demo");
}

const BLUE = "#7aa7e6";
const VIOLET = "#a99be8";

/** A sample screen (data from the `V3-Arrangement` mockup). */
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
  /** External monitor: what it shows depends on the scene. */
  external?: boolean;
  /** Disconnected: does not take part in the arrangement. */
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

/** A line of the mockups (`y` heights on a canvas of `h`) as 0–100 values. */
const fromY = (h: number, ys: number[]) => ys.map((y) => ((h - y) / h) * 100);

const BUILTIN_14: DemoScreen = { id: "mbp", name: "MacBook Pro", short: "Mac", ask: "MacBook Pro", w: 12, h: 7.8, size: '14"', res: "1512 × 982", ratio: "16:10", scale: "200 % (Retina)" };
const G34: DemoScreen = { id: "g34", name: 'Odyssey G5 34"', short: '34"', ask: 'the 34"', w: 31.4, h: 13.1, size: '34"', res: "3440 × 1440", ratio: "21:9", scale: "100 %", external: true };
const G32: DemoScreen = { id: "g32", name: 'Odyssey G5 32"', short: '32"', ask: 'the 32"', w: 27.9, h: 15.7, size: '32"', res: "2560 × 1440", ratio: "16:9", scale: "—", external: true, off: true };
const U27: DemoScreen = { id: "u27", name: 'UltraFine 27"', short: '27"', ask: 'the 27"', w: 23.5, h: 13.2, size: '27"', res: "2560 × 1440", ratio: "16:9", scale: "100 %", external: true };
const STUDIO: DemoScreen = { id: "sd27", name: "Studio Display", short: "Studio", ask: "the Studio Display", w: 23.5, h: 13.2, size: '27"', res: "2560 × 1440", ratio: "16:9", scale: "200 %", external: true };

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
    { id: "docker", kind: "docker", name: "Docker", default: false, ready: true, detail: "arm64 · no GPU" },
  ],
  inputDevices: [{ name: "Built-in keyboard" }, { name: "Trackpad" }, { name: "Keychron K3 · MX Master 3S", via: "through the hub" }],
  live: {
    cpu: { percent: 18, temp: 48, history: fromY(34, [28, 26, 29, 24, 27, 25, 28, 26, 27]) },
    ram: { used: 12.4 * GIB, total: 24 * GIB, history: fromY(22, [14, 13, 13, 12, 12, 11, 12, 12, 11]) },
    gpu: { percent: 6, vramUsed: 1.2 * GIB, vramTotal: null, temp: 44, fans: null, history: fromY(26, [22, 21, 22, 19, 22, 21, 22, 21, 22]) },
    disk: { free: 231 * GIB, total: 1024 * GIB },
    battery: { percent: 84, plugged: true },
  },
};

const DESKTOP: Spec = {
  name: "PC-Desktop",
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
    { id: "docker", kind: "docker", name: "Docker Desktop", default: false, ready: true, detail: "GPU: --gpus all available" },
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
  name: "PC-Workshop",
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
  // "the MacBook" (the mockup names it that way); any other Mac, with its full name ("the Mac mini"), without cutting it.
  const shown = name.startsWith("MacBook") ? "the MacBook" : `the ${name}`;
  // With a macOS permission missing, the page shows it in its own row ("Permissions", like
  // `F-MachineMac`); the module goes on with what it can do.
  const list: ModuleView[] = [
    { id: "input", name: "Keyboard and mouse", state: "active", detail: mac ? "Capture with CGEventTap and injection with CGEventPost" : "Capture and injection (low-level hooks, SendInput)", switchable: true },
    { id: "clipboard", name: "Clipboard", state: "active", detail: "Text, HTML and image", switchable: true },
    mac
      ? { id: "monitors", name: "Monitors", state: "off", detail: `Over HDMI there is a DDC channel only while the monitor shows ${shown}. Today ${otherName} controls them.`, switchable: true }
      : { id: "monitors", name: "Monitors", state: "active", detail: 'DDC channel over DisplayPort to 2 monitors. Owner of the 34".', switchable: true },
    {
      id: "power",
      name: "Power",
      state: mac ? "off" : "active",
      detail: mac ? "It can wake the others over Wake-on-LAN. It does not power itself off." : `It powers itself off. It wakes over Wake-on-LAN from ${otherName}.`,
      switchable: false,
    },
    mac
      ? adminPending
        ? { id: "tunnels", name: "Remote compute", state: "off", detail: `${envs} environments · the administrator service still needs installing`, switchable: true }
        : { id: "tunnels", name: "Remote compute", state: "active", detail: `${envs} environments · admin (LaunchDaemon installed)`, switchable: true }
      : { id: "tunnels", name: "Tunnels", state: "active", detail: `Receives Claude tunnels in ${envs} environments, as administrator.`, switchable: true },
  ];
  if (!mac) list.push({ id: "secure_desktop", name: "Protected screens", state: "active", detail: "The mouse and keyboard also work on UAC prompts and at the sign-in screen.", switchable: true });
  return list;
}

const ctrlAlt = (n: number): string[] => ["ControlLeft", "AltLeft", `Digit${n}`];

/** The demo monitors: the external ones of each machine. The disconnected ones are not compatible (D11). */
function demoMonitors(sa: Spec, sb: Spec): MonitorUi[] {
  const out: MonitorUi[] = [];
  for (const [owner, spec] of [["b", sb] as const, ["a", sa] as const]) {
    for (const d of spec.screens.filter((x) => x.external)) {
      if (d.off) {
        out.push({ id: d.id, name: d.name, compat: { ok: false, reason: "It joins only once its firmware meets the standard." }, connected: false, owner: null, input: null, ports: {}, shows: null, events: [] });
        continue;
      }
      const input: MonitorInput = owner === "a" ? "hdmi" : "display_port";
      // The demo time is fixed (local time): screenshots do not depend on the time zone.
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
          { at: at(14, 2), kind: "switch", text: "Input → DP, requested by Tandem" },
          { at: at(13, 47), kind: "button", text: "Input → HDMI, with the buttons" },
          { at: at(13, 47), kind: "signal", text: "HDMI signal detected" },
        ],
      });
    }
  }
  return out.sort((x, y) => Number(y.compat.ok) - Number(x.compat.ok));
}

/** The demo scenes over the first compatible monitor (like the mockup). */
export function demoScenes(a: string, b: string, monitors: MonitorUi[], mac: boolean, pcOnly = false, workOnA = false): SceneView[] {
  const m = monitors.find((x) => x.compat.ok)?.id;
  const raw: SceneJson[] = [
    { id: "work", name: "Work", monitors: m ? [[m, workOnA ? "a" : "b"]] : [], shortcut: ctrlAlt(1) },
    { id: "only-a", name: `${a.split(" ")[0]} only`, monitors: m ? [[m, "a"]] : [], shortcut: ctrlAlt(2) },
    { id: "game", name: pcOnly ? "PC only" : "Game", monitors: m ? [[m, "b"]] : [], shortcut: ctrlAlt(3) },
  ];
  return raw.map((r) => sceneView(r, { monitors, name: (x) => (x === "a" ? a : b), mac }));
}

const MIN = 60_000;
/** Half a minute extra: the times read "34 m" even if an instant passes between building the demo and drawing it. */
const PAD = 20_000;

interface DemoProc {
  cmd: string;
  /** How long ago it started (ms). */
  ago: number;
  pid: number;
  log: string;
}

const p = (cmd: string, min: number, pid: number, log: string): DemoProc => ({ cmd, ago: min * MIN + PAD, pid, log });

/** The processes running in each demo tunnel (those of `V3-Compute`) and their output. */
const DEMO_PROCS: Record<string, DemoProc[]> = {
  t97: [
    p("cargo test -p tandem-monitor", 2, 4127, "   Compiling tandem-monitor v0.3.0\n     Running unittests src/lib.rs\ntest probe_never_sends_vendor_codes ... ok\ntest hotplug_adds_monitor ... ok\ntest owner_failover_when_pc_sleeps ... ok\ntest verdict_failed_rolls_back_scene ... "),
    p("monitor-events serve --sim 2", 30, 3810, "listening on 127.0.0.1:8765\nGET /api/monitors 200\nSSE monitor-added lc34g55t-9c1e\nSSE input-changed lc34g55t-9c1e dp\nSSE switch-done lc34g55t-9c1e 1.8s\n"),
    p("cargo watch -x check -p tandem-monitor", 25, 3902, "[Running cargo check]\n    Finished dev in 0.9s\n[Finished running. Exit status: 0]\n"),
    p("tandemd --sim --monitors 2", 22, 3955, "monitors: 2 detected (1 compatible)\nowner of lc34g55t-9c1e: pc-sim\n"),
    p("pnpm --filter tandem-ui dev", 20, 3988, "VITE v6 ready in 412 ms\n➜ Local: http://localhost:1420/\n"),
    p("tail -f ~/.tandem/logs/tandemd.log", 19, 4001, "14:06:12 monitor lc34g55t-9c1e input=dp (firmware)\n14:06:14 scene “Work” applied\n"),
  ],
  t101: [
    p("cargo test --workspace", 3, 5210, "     Running tests/two_processes.rs\ntest tunnel_survives_link_drop ... ok\ntest tunnel_many_processes ... ok\ntest pairing_with_real_quic ... "),
    p("tandemd --sim --peers 2", 15, 5102, "peer mac-sim connected over loopback\ntunnel t-01 open · 0.1 ms\n3 live processes in t-01\n"),
  ],
  pr: [],
  lora: [
    p("uv run train.py --config style.toml", 41, 2290, "epoch 7/20  step 1860/5200  loss 0.0398\nsaved out/style-e7.safetensors\nepoch 8/20  step 1880/5200  loss 0.0405\n"),
    p("python main.py --listen 127.0.0.1", 60, 2104, "Starting server on 127.0.0.1:8188\nGPU: RTX 5080 · 16 GB\nprompt executed in 4.2 s\n"),
    p("tensorboard --logdir out", 40, 2301, "TensorBoard 2.18 at http://localhost:6006/\n"),
  ],
  comfy: [],
};

/** A tunnel with many processes (`tunnels=many`): the list has to withstand them with scrolling. */
const MANY_COMMANDS = [
  "cargo build --workspace",
  "cargo test -p tandem-link",
  "cargo clippy --workspace",
  "pnpm --filter tandem-ui dev",
  "pnpm --filter tandem-ui test --watch",
  "tandemd --sim --peers 2",
  "tail -f ~/.tandem/logs/tandemd.log",
  "monitor-events serve --sim 2",
  "uv run train.py --config style.toml",
  "python main.py --listen 127.0.0.1",
  "tensorboard --logdir out",
  "nvidia-smi -l 5",
];
const MANY: DemoProc[] = MANY_COMMANDS.map((c, i) => p(c, 3 + i * 4, 6000 + i, `$ ${c}\nready\n`));

/** The tunnels demo: the five of `V3-Compute`, none (`tunnels=none`) or one with 12 processes (`tunnels=many`). */
export function demoTunnels(now: number, which: DemoOptions["tunnels"] = "normal", oses: Record<Machine, Os> = { a: "macos", b: "windows" }): TunnelView[] {
  if (which === "none") return [];
  // The environment comes from the target machine's system (G2): on Windows, WSL or Windows itself; on macOS, zsh.
  const env = (to: Machine, windows: string) => (oses[to] === "macos" ? "zsh" : windows);
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
    t("t97", "tandem", "~/…/apps/tandem", "Monitors crate (T-097)", "task/T-097", "a", "b", "busy", 34, env("b", "WSL · Ubuntu"), { commands: 22, traffic: { up: 120e6, down: 4e6 } }),
    t("t101", "tandem", "~/…/apps/tandem", "Tunnels in tandemd (T-101)", "task/T-101", "a", "b", "busy", 18, env("b", "WSL · Ubuntu"), { commands: 12, traffic: { up: 64e6, down: 3e6 } }),
    t("pr", "tandem", "~/…/apps/tandem", `Review PR #66 on ${oses.b === "macos" ? "macOS" : "Windows"}`, "develop", "b", "a", "idle", 41, env("a", "WSL · Ubuntu"), { commands: 9, ports: 1, idleSince: now - 9 * MIN - PAD, traffic: { up: 2e6, down: 18e6 } }),
    t("lora", "ia-local", "~/…/ia-local", "Train style LoRA", "main", "a", "b", "busy", 134, env("b", "WSL · Ubuntu"), { commands: 37, ports: 1, traffic: { up: 2.1e9, down: 38e6 } }),
    t("comfy", "ia-local", "~/…/ia-local", "Try ComfyUI workflow", "main", "a", "b", "closed", 0, env("b", "Windows"), { commands: 4, closedAt: now - 60 * MIN, traffic: { up: 12e6, down: 1e6 } }),
  ];
  if (which === "many") return [{ ...list[0], processes: MANY.length }, ...list.slice(1)];
  // `V3-Tray` draws the PR review as a third project ("notes"); `V3-Compute` leaves it in "tandem".
  // `F-Palette` lists one tunnel per project, with the project as the title.
  if (which === "palette") return [{ ...list[3], title: "ia-local" }, { ...list[0], title: "tandem" }];
  // `V3-Tray`: two active projects with one tunnel each and the "notes" idle.
  if (which === "tray") return [{ ...list[3], title: "ia-local" }, { ...list[0], title: "tandem" }, { ...list[2], project: "notes", projectPath: "~/…/notes" }];
  if (which === "notes") return list.map((x) => (x.id === "pr" ? { ...x, project: "notes", projectPath: "~/…/notes" } : x));
  return list;
}

/** The demo processes of a tunnel, as the agent would give them. */
export function demoProcesses(id: string, now: number, which: DemoOptions["tunnels"] = "normal"): ProcessRow[] {
  const procs = which === "many" && id === "t97" ? MANY : (DEMO_PROCS[id] ?? []);
  return procs.map((q, i) => ({ id: i + 1, command: q.cmd, pid: q.pid, startedAt: now - q.ago, state: "running", code: null }));
}

/** The demo output of a process (whole: it has no cursor to advance). */
export function demoLog(id: string, process: number, which: DemoOptions["tunnels"] = "normal"): string {
  const procs = which === "many" && id === "t97" ? MANY : (DEMO_PROCS[id] ?? []);
  return procs[process - 1]?.log ?? "";
}

/** The demo time (fixed: deterministic screenshots). Path changes are dated from here. */
export const DEMO_NOW = 1_763_400_000_000;

/** What each path gives in the demo, in Gbps (the numbers of `V3-Network`). */
const DEMO_GBPS: Record<LinkKind, number> = { usb4: 9.4, ethernet: 2.3, wifi: 0.41 };

const HISTORY = 60;
/** Second of the history in which the cable was disconnected and in which it came back. */
const CUT_AT = 23;
const BACK_AT = 31;

const round = (n: number, d = 10) => Math.round(n * d) / d;

/**
 * The input p99 of each second (ms): the low line of `V3-Network` and, between the cut and the
 * cable's return, the peak, which Wi-Fi carries.
 */
const DEMO_INPUT = [
  1.2, 1.22, 1.25, 1.27, 1.29, 1.27, 1.23, 1.18, 1.14, 1.11, 1.18, 1.25, 1.31, 1.38, 1.37, 1.32, 1.28, 1.23, 1.21, 1.23, 1.25, 1.27, 1.3, 2.15, 3.12,
  4.09, 5.3, 5.6, 3.9, 2.82, 2.05, 1.3, 1.28, 1.25, 1.23, 1.21, 1.21, 1.24, 1.26, 1.28, 1.3, 1.33, 1.35, 1.37, 1.39, 1.37, 1.32, 1.28, 1.23, 1.21,
  1.23, 1.25, 1.28, 1.3, 1.3, 1.3, 1.3, 1.3, 1.3, 1.3,
];

/**
 * Second `i` of a path's history (deterministic). USB-C carries the input except
 * between the cut and the return, when Wi-Fi carries it: that is where the chart peak is.
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

/** What is known about each link in the demo (the texts of `V3-Network`). */
const DEMO_DETAIL: Record<LinkKind, string> = { usb4: "USB4 · 20 Gbps", ethernet: "2.5 GbE", wifi: "Wi-Fi 7 · home" };

/** The demo network: primary USB-C, Ethernet with no cable and backup Wi-Fi (`V3-Network`). */
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
  "no-agent": () => "The Tandem agent is not running on this machine",
  "peer-off": (peer) => `${peer} is off`,
  "link-down": (peer) => `No link with ${peer}`,
  permissions: () => "Permissions are missing on this machine",
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
    binary: "Grant the permission to this binary: /Applications/Tandem.app/Contents/MacOS/tandemd (pid 4242)",
  };
}

/** As in the mockup: the other machine to the left of the 34", aligned at the bottom. */
export const DEMO_ARRANGEMENT: Arrangement = { aSide: "left", align: "end" };

/** The sample screens: A's first (like the mockup), the disconnected ones last. */
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
        off.push({ ...rest, id, machine: null, x: 0, y: 0, usable: false, line: "Disconnected" });
        continue;
      }
      const shows = external && scene?.shows === m ? ` · scene ${scene.name}` : "";
      screens.push({ ...rest, id, machine: m, x: 0, y: 0, usable: true, line: `${spec.name}${shows}`, external: external === true, native });
    }
  }
  return { screens: [...screens, ...off], arrangement: { ...DEMO_ARRANGEMENT } };
}

/** With the desk paired, the other machine goes over the cable; before that it is only found over the network (Wi-Fi). */
const PAIRED_LINK: LinkView = { kind: "usb4", gbps: 9.4, ms: 0.3, inputP99: 1.9 };
/** `V3-Tray` says 0.4 ms (the Desk, 0.3). */
const TRAY_LINK: LinkView = { ...PAIRED_LINK, ms: 0.4 };
const UNPAIRED_LINK: LinkView = { kind: "wifi", gbps: 0.41, ms: 3.8, inputP99: null };
const NO_LINK: LinkView = { kind: null, gbps: null, ms: null, inputP99: null };

/** The simulated desk for some options (pure: same options, same desk). */
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
      resources: power === "on" ? { ...s.resources, ...(o.tray && s.os === "macos" ? { gpu: { percent: 18, label: "18 %" } } : {}), history: [...s.resources.history] } : null,
      modules: modulesOf(s.os, s.name, otherName, s.environments.length, o.admin === "pending"),
      system: s.system,
      hardware: s.hardware,
      environments: s.environments.map((e) => ({ ...e })),
      live: power === "on" ? s.live : null,
      wake: here ? null : { mac: "aa:bb:cc:dd:ee:ff" },
      ...(s.inputDevices ? { inputDevices: s.inputDevices } : {}),
    };
  };
  const monitors = demoMonitors(sa, sb).map((m) => (o.txn === "fail" && m.compat.ok ? { ...m, input: "hdmi" as const, shows: "a" as const, owner: "b" as const } : m));
  const scenes = demoScenes(sa.name, sb.name, monitors, (o.here === "a" ? sa : sb).os === "macos", o.pcOnly, o.palette);
  const linkUp = o.trouble !== "peer-off" && o.trouble !== "link-down" && o.trouble !== "no-agent";
  return {
    ready: true,
    deskName: "My desk",
    prefs: { cmdCtrl: true, dragFiles: false, clipboard: true },
    lockShortcut: ["ScrollLock"],
    locked: false,
    trouble: o.trouble,
    troubleText: TROUBLE_TEXT[o.trouble](peerSpec.name),
    here: o.here,
    machines: { a: mk("a", sa, sb.name), b: mk("b", sb, sa.name) },
    link: linkUp ? (o.unpaired ? UNPAIRED_LINK : o.tray ? TRAY_LINK : PAIRED_LINK) : NO_LINK,
    network: o.txn && linkUp ? { ...demoNetwork(linkUp), measuring: "usb4" } : demoNetwork(linkUp),
    tunnels: demoTunnels(o.still ? DEMO_NOW : Date.now(), o.tunnels, { a: sa.os, b: sb.os }),
    scenes,
    activeScene: "work",
    monitors,
    pair: {
      peer: o.unpaired ? { kind: "unpaired" } : { kind: "paired", name: peerSpec.name, link: linkUp ? "usb4" : null },
      pairing: o.pairing === "code" ? { kind: "comparing", code: DEMO_CODE, peer: peerSpec.name, confirmed: false } : { kind: "idle" },
    },
    permissions: permissionsFor((o.here === "a" ? sa : sb).os, o.trouble === "permissions" || o.perms === "missing", o.perms === "missing"),
    layout: demoLayout(sa, sb, scenes, "work"),
  };
}

/** How long the simulated crossing takes when nobody said how it ends. */
const CROSS_MS = 2500;

const PAIR_FOUND_MS = 1400;
const PAIR_OTHER_MS = 1200;

/** How long the other machine takes to "edit" the desk when the demo asks for it. */
const PEER_EDIT_MS = 1200;

const CURSOR_MS = 33;
const TICK_MS = 1500;
const RESCAN_MS = 900;
/** How long the bandwidth measurement takes in the demo. */
const DEMO_MEASURE_MS = 1500;

/** How long each step of a simulated transaction takes. */
const STEP_MS = 700;

/** A simulated source: it responds to actions as the agent would. */
export class DemoSource implements Source {
  readonly demo = true;
  private desk: DeskView;
  private emit: (d: DeskView) => void = () => {};
  private timer: ReturnType<typeof setInterval> | undefined;
  private topics: Topic[] = [];
  /** What was asked live, for the tests. */
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

  /** The progress of a file copy, as the agent would send it. */
  simulateTransfer(t: TransferView): void {
    this.onTransfer?.(t);
  }

  /** The other machine changed the desk (D5): what the agent's `DeskChanged` would do. */
  simulateChange(c: DeskChange): void {
    this.onChange?.(c);
  }

  /** Desk version in the demo: each change raises it. */
  rev = 1;
  /** The desk as it was at each version, so "Undo" really restores it. */
  private readonly history = new Map<number, DeskView>();

  /** Saves the current desk as the current version and raises the version; returns the previous one. */
  private bump(): number {
    const before = this.rev;
    this.history.set(before, this.desk);
    this.rev += 1;
    return before;
  }

  /** A change made from the other machine (arrangement): it is applied and announces itself with "Undo". */
  peerEdit(summary = "Keyboard and mouse: the shared clipboard is off"): void {
    const undoTo = this.bump();
    this.set({ ...this.desk, prefs: { ...this.desk.prefs, clipboard: !this.desk.prefs.clipboard } });
    this.simulateChange({ by: this.desk.here === "a" ? "b" : "a", rev: this.rev, summary, undoTo });
  }

  async setLockShortcut(lock: string[]): Promise<ActResult> {
    this.set({ ...this.desk, lockShortcut: lock });
    return { ok: true, text: "Lock shortcut saved" };
  }

  async setPrefs(p: { cmdCtrl: boolean; dragFiles: boolean }): Promise<ActResult> {
    this.bump();
    this.set({ ...this.desk, prefs: { ...this.desk.prefs, ...p } });
    return { ok: true };
  }

  async undoDesk(rev: number): Promise<ActResult> {
    const before = this.history.get(rev);
    if (!before) return { ok: false, reason: "That desk version is gone" };
    this.bump();
    this.set(before);
    return { ok: true, text: "Change undone" };
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
    // Resources and the network change by themselves, and only if the visible screen shows them.
    const live = topics.includes("resources") || topics.includes("network");
    if (!this.opts.still && live) this.timer = setInterval(() => this.tick(), TICK_MS);
  }

  /** Advances a sample of what the visible screen shows: resources and network. */
  tick(): void {
    if (this.topics.includes("resources")) this.tickResources();
    if (this.topics.includes("network")) this.tickNetwork();
  }

  /** The history of each responding path advances a second. */
  private tickNetwork(): void {
    const paths = this.desk.network.paths.map((p) => {
      if (p.state !== "primary" && p.state !== "backup") return p;
      // Normal traffic (the seconds before the cut), in a cycle.
      const point = demoPoint(p.kind, this.ticks % CUT_AT);
      return { ...p, rttMs: point.rttMs ?? p.rttMs, history: [...p.history.slice(1), point] };
    });
    this.ticks += 1;
    this.set({ ...this.desk, network: { ...this.desk.network, paths, now: DEMO_NOW + this.ticks * 1000 } });
  }

  private ticks = 0;

  /** Moves the resources a little: the sparkline advances a sample. */
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

  // ---- Monitors, scenes and their progress ---------------------------------------------

  private txnListeners = new Set<(e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void>();
  private nextTxn = 1;
  private txnTimers: ReturnType<typeof setTimeout>[] = [];

  /** The source time: fixed in screenshots (fixed progress or still demo), otherwise the clock's. */
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

  /** The progress of "Game" at a fixed point, with the times of `F-Transactions`. */
  private replay(emit: (e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void, at: "run" | "ok" | "fail"): void {
    const g34 = this.desk.monitors.find((m) => m.compat.ok);
    const steps: Step[] = [
      { step: "show_machine", monitor: g34?.id ?? "g34", machine: "b" },
      { step: "geometry" },
    ];
    const t0 = DEMO_NOW;
    const txn = 1;
    // The other two cards of `F-Transactions`: the PC powering on has been waiting 18 s
    // (the source clock runs alongside) and the USB-C link is being measured (in `demoDesk`).
    const boot = this.now() - 18_000;
    emit({ type: "started", txn: 2, steps: [{ step: "power_on", machine: "b" }] }, boot);
    emit({ type: "step_started", txn: 2, index: 0 }, boot + 100);
    emit({ type: "started", txn, steps }, t0, { title: "Applying the scene “Game”", doneText: "Scene “Game” active." });
    emit({ type: "step_started", txn, index: 0 }, t0 + 100);
    if (at === "run") return;
    if (at === "fail") {
      emit({ type: "step_failed", txn, index: 0, error: "the monitor showed no image over DisplayPort; it stays on HDMI" }, t0 + 3000);
      return;
    }
    emit({ type: "step_done", txn, index: 0 }, t0 + 1900);
    emit({ type: "step_started", txn, index: 1 }, t0 + 1900);
    emit({ type: "step_done", txn, index: 1 }, t0 + 2300);
    emit({ type: "committed", txn, state: { on: { a: true, b: true }, scene: "game" } }, t0 + 2300);
  }

  private emitTxn(e: TxnView): void {
    for (const l of this.txnListeners) l(e);
  }

  /**
   * Runs a simulated transaction: each step takes a little and announces itself. With `still` everything happens
   * instantly. `failAt` makes that step fail and undoes the previous ones.
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
          this.emitTxn({ type: "step_failed", txn, index: i, error: "the monitor showed no image over DisplayPort; it stays on HDMI" });
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
    if (!m) return { ok: false, reason: "That monitor is not known" };
    if (!m.compat.ok) return { ok: false, reason: `${m.name} is not supported: nothing is written to it` };
    if (!m.connected) return { ok: false, reason: `${m.name} is disconnected` };
    const to = (["a", "b"] as Machine[]).find((x) => m.ports[x] === input) ?? null;
    this.runTxn(
      [{ step: "switch_monitor", monitor: id, input }],
      () => this.setMonitor(id, input, to),
      () => {},
    );
    return { ok: true };
  }

  /** The real input of a monitor changed (by the requested change or by the scene). */
  private setMonitor(id: string, input: MonitorInput, shows: Machine | null): void {
    const monitors = this.desk.monitors.map((m) => {
      if (m.id !== id) return m;
      // With `still` the event is not added: it would carry the clock time (deterministic screenshots).
      const events = this.opts.still ? m.events : [{ at: Date.now(), kind: "switch" as const, text: `Input → ${input === "hdmi" ? "HDMI" : "DP"}, requested by Tandem` }, ...m.events].slice(0, 3);
      return { ...m, input, shows, events };
    });
    this.set({ ...this.desk, monitors });
  }

  async applyScene(id: string): Promise<ActResult> {
    const scene = this.desk.scenes.find((s) => s.id === id);
    if (!scene) return { ok: false, reason: "That scene does not exist" };
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
    if (isNew && exists) return { ok: false, reason: `There is already a scene with the id “${id}”` };
    if (!isNew && !exists) return { ok: false, reason: "That scene no longer exists" };
    const scenes = isNew ? [...this.desk.scenes, view] : this.desk.scenes.map((s) => (s.id === id ? view : s));
    this.set({ ...this.desk, scenes });
    return { ok: true, text: `Scene “${scene.name}” saved` };
  }

  private freeId(name: string): string {
    const base = name.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "scene";
    let id = base;
    for (let n = 2; this.desk.scenes.some((s) => s.id === id); n++) id = `${base}-${n}`;
    return id;
  }

  async forgetMonitor(id: string): Promise<ActResult> {
    const m = this.desk.monitors.find((x) => x.id === id);
    if (!m) return { ok: false, reason: "That monitor is not known" };
    if (m.compat.ok || m.connected) return { ok: false, reason: `${m.name} is in use: only unsupported ones that are not present can be forgotten` };
    this.set({ ...this.desk, monitors: this.desk.monitors.filter((x) => x.id !== id) });
    return { ok: true, text: `${m.name} forgotten` };
  }

  async setPower(machine: Machine, on: boolean): Promise<ActResult> {
    const m = this.desk.machines[machine];
    if (!m) return { ok: false, reason: "That machine is not known" };
    if (m.here) return { ok: false, reason: "This machine cannot be powered off from here" };
    const spec = demoDesk(this.opts).machines[machine];
    this.set({
      ...this.desk,
      machines: { ...this.desk.machines, [machine]: { ...m, power: on ? "on" : "off", resources: on ? (spec?.resources ?? null) : null, live: on ? (spec?.live ?? null) : null } },
    });
    return { ok: true, text: on ? `${m.name} powered on` : `${m.name} powered off` };
  }

  /** Instantly measures what each path gives in the demo. */
  async measureLink(path?: LinkKind): Promise<ActResult> {
    const kind = path ?? this.desk.link.kind;
    const target = this.desk.network.paths.find((p) => p.kind === kind);
    if (!kind || !target || (target.state !== "primary" && target.state !== "backup")) return { ok: false, reason: "No link with the other machine over that path" };
    if (!this.opts.still) {
      // The real test lasts 3 s: the screen shows which path is being measured.
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
    if (!m || !mod) return { ok: false, reason: "That module does not exist" };
    if (mod.state === "unavailable") return { ok: false, reason: mod.detail };
    const modules = m.modules.map((x) => (x.id === module ? { ...x, state: on ? ("active" as const) : ("off" as const) } : x));
    const prefs = module === "clipboard" ? { ...this.desk.prefs, clipboard: on } : this.desk.prefs;
    this.set({ ...this.desk, prefs, machines: { ...this.desk.machines, [machine]: { ...m, modules } } });
    return { ok: true };
  }

  /** Rescanning takes a while, as on the real machine; what is found returns with the desk. */
  async rescan(machine: Machine): Promise<ActResult> {
    const m = this.desk.machines[machine];
    if (!m) return { ok: false, reason: "That machine is not known" };
    if (m.power !== "on") return { ok: false, reason: `${m.name} is off` };
    await new Promise((r) => setTimeout(r, RESCAN_MS));
    const now = this.desk.machines[machine];
    if (now) this.set({ ...this.desk, machines: { ...this.desk.machines, [machine]: { ...now, environments: now.environments.map((e) => ({ ...e })) } } });
    return { ok: true };
  }

  async setDeskName(name: string): Promise<ActResult> {
    const n = name.trim();
    if (n.length > 40) return { ok: false, reason: "the name cannot exceed 40 characters" };
    this.set({ ...this.desk, deskName: n || "My desk" });
    return { ok: true };
  }

  async setWol(mac: string | null): Promise<ActResult> {
    if (mac !== null && !/^([0-9a-f]{2}[:-]){5}[0-9a-f]{2}$/i.test(mac.trim())) {
      return { ok: false, reason: `invalid MAC address “${mac}”: 6 bytes expected, like aa:bb:cc:dd:ee:ff` };
    }
    const m = this.desk.machines.b;
    if (!m?.wake) return { ok: false, reason: "this machine cannot be powered on over the network" };
    this.set({ ...this.desk, machines: { ...this.desk.machines, b: { ...m, wake: { mac: mac === null ? null : mac.trim().toLowerCase().replaceAll("-", ":") } } } });
    return { ok: true };
  }

  /** The same thing the desk validates: non-empty and short name, a color from the palette and distinct. */
  async editMachine(machine: Machine, edit: { name?: string; color?: string }): Promise<ActResult> {
    const m = this.desk.machines[machine];
    if (!m) return { ok: false, reason: `machine ${machine.toUpperCase()} is not known yet` };
    let { name, color } = m;
    if (edit.name !== undefined) {
      const c = checkName(edit.name);
      if (!c.ok) return { ok: false, reason: c.reason };
      name = c.name;
    }
    if (edit.color !== undefined) {
      if (!/^#[0-9a-f]{6}$/i.test(edit.color)) return { ok: false, reason: `“${edit.color}” is not a #rrggbb color` };
      const other = this.desk.machines[machine === "a" ? "b" : "a"];
      if (other && other.color.toLowerCase() === edit.color.toLowerCase()) return { ok: false, reason: `that color already belongs to ${other.name}` };
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

  /** Simulates the other machine: the code appears and, on confirming, it confirms too. */
  async setArrangement(a: Arrangement): Promise<ActResult> {
    if (this.desk.trouble === "no-agent") return { ok: false, reason: "The Tandem agent is not running on this machine" };
    this.set({ ...this.desk, layout: { ...this.desk.layout, arrangement: { ...a } } });
    return { ok: true, text: "Arrangement saved" };
  }

  private listeners = new Set<(m: Machine) => void>();

  /** Listen for keys on each machine; the demo receives them from `heardKey` (the tests) or from the window. */
  async watchKeyProbe(onHeard: (m: Machine) => void): Promise<() => void> {
    this.listeners.add(onHeard);
    return () => void this.listeners.delete(onHeard);
  }

  /** A key heard on `m`. */
  heardKey(m: Machine): void {
    for (const f of [...this.listeners]) f(m);
  }

  private crossers = new Set<(s: CrossSample) => void>();

  /** The crossing test: with `cross=…` it ends as requested; otherwise it crosses fine after a few seconds. */
  async watchCrossing(onCross: (s: CrossSample) => void): Promise<() => void> {
    this.crossers.add(onCross);
    const { here, layout } = this.desk;
    const edge = exitSide(layout.arrangement, here);
    const how = this.opts.cross;
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (how === "ok" || how === "wrong") {
      // The wrong edge: one of the two that are neither the arrangement's nor the opposite.
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
   * The live cursor: follows the mockup path (7 s, from the MacBook to the 34" and back) and
   * announces each crossing. With `still` (or without allowed motion) it sends only the starting point.
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

  /** A cursor crossing (what the input engine would report). */
  cross(s: CrossSample): void {
    for (const f of [...this.crossers]) queueMicrotask(() => f(s));
  }

  /** With `cross=timeout` the test runs out of time right away. */
  get crossTimeoutMs(): number | undefined {
    return this.opts.cross === "timeout" ? 0 : undefined;
  }

  async pairAction(kind: PairActionKind): Promise<ActResult> {
    clearTimeout(this.pairTimer);
    const peerName = this.desk.machines[this.desk.here === "a" ? "b" : "a"]?.name ?? "the other machine";
    const p = this.desk.pair.pairing;
    switch (kind) {
      case "start_pairing":
        this.setPairing({ kind: "searching" });
        this.pairTimer = setTimeout(() => this.setPairing({ kind: "comparing", code: DEMO_CODE, peer: peerName, confirmed: false }), PAIR_FOUND_MS);
        return { ok: true };
      case "confirm_pairing":
        if (p.kind !== "comparing") return { ok: false, reason: "There is no code to confirm" };
        this.setPairing({ ...p, confirmed: true });
        this.pairTimer = setTimeout(() => {
          if (this.opts.pairing === "reject") this.setPairing({ kind: "failed", reason: `it was rejected on ${peerName}` });
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

  /** The demo has no input engine: locking only changes the state shown. */
  async lockHere(on: boolean): Promise<ActResult> {
    this.set({ ...this.desk, locked: on });
    return { ok: true, text: on ? "Cursor locked here" : "Lock removed" };
  }

  async requestPermissions(): Promise<ActResult> {
    return { ok: true };
  }

  /** In the demo, opening the panel is equivalent to the user granting it. */
  async openPermission(p: Permission): Promise<ActResult> {
    const perms = this.desk.permissions;
    const rows = perms.rows.map((r) => (r.id === p && r.state === "missing" ? { ...r, state: "granted" as const } : r));
    const trouble = rows.some((r) => r.state === "missing") ? this.desk.trouble : this.desk.trouble === "permissions" ? "none" : this.desk.trouble;
    this.set({ ...this.desk, trouble, troubleText: trouble === "none" ? "" : this.desk.troubleText, permissions: { ...perms, rows } });
    return { ok: true };
  }

  async restartAgent(): Promise<ActResult> {
    return { ok: true, text: "tandemd is restarting: permissions are re-checked in a few seconds" };
  }

  private adminDone = false;

  async adminStatus(): Promise<AdminView> {
    const off = this.opts.admin === "off";
    const state: AdminView["state"] = this.adminDone ? "ok" : (this.opts.admin ?? "ok");
    return { state, canInstall: state !== "ok" && !off, note: off ? "per-user install: install Tandem under Program Files for the service" : "" };
  }

  async installAdmin(): Promise<AdminView> {
    if (this.opts.admin === "off") throw new Error("per-user install: not applicable");
    this.adminDone = true;
    return this.adminStatus();
  }

  private setupShown = false;

  async setup(): Promise<SetupView> {
    const needed = this.desk.permissions.needed;
    return { available: needed, show: !!this.opts.firstRun && !this.setupShown, permissions: needed ? ["accessibility", "input_monitoring", "local_network"] : [], agent: null };
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
    // The demo gives all the output at once; with the cursor at the end there is nothing new.
    return { text: from.stdout > 0 ? "" : text, next: { stdout: text.length, stderr: 0 }, done: false };
  }

  // Simulated updates (`F-Settings`): both on 0.3.0 and, when searching, 0.3.1 appears.
  private upd: UpdateView = { seq: 0, here: "0.3.0", peer: "0.3.0", paired: true, available: null, checking: false, step: null, error: null, auto: true };
  private onUpd: ((v: UpdateView) => void) | null = null;

  private setUpd(v: UpdateView): UpdateView {
    this.upd = { ...v, seq: this.upd.seq + 1 };
    v = this.upd;
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

  /** Nothing is installed in the demo: the first step is shown and stays there. */
  async updateAll(): Promise<UpdateView> {
    if (!this.upd.available) throw new Error("there is no new version: check first");
    return this.setUpd({ ...this.upd, step: "ask_peer" });
  }

  async setAutoUpdate(on: boolean): Promise<UpdateView> {
    return this.setUpd({ ...this.upd, auto: on });
  }

  /** Everything can be simulated; measuring, only with a responding path. */
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
