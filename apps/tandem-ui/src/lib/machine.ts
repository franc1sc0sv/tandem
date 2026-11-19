// The Machine page (T-115, `V3-Machine`/`F-MachineMac`): texts, units and order of what it
// shows, and how the live view is built from the agent's samples. Pure: tested without a
// screen.

import type { Permission, ResourceSample } from "./model";
import type { EnvView, HardwareView, LiveView, MachineView, ModuleState, Power, Resources } from "./store/types";

const GIB = 2 ** 30;

/** Samples kept per line (one per second with the agent). */
export const HISTORY = 30;

/** From here the VRAM counts as almost full (the amber notice). */
export const VRAM_WARN = 0.85;

/** The machine color palette (D17), with the name of each. */
export const PALETTE: { color: string; name: string }[] = [
  { color: "#7aa7e6", name: "Blue" },
  { color: "#a99be8", name: "Violet" },
  { color: "#5fbfb0", name: "Teal" },
  { color: "#d9a75a", name: "Amber" },
];

/** The maximum long name the desk accepts (`tandem_core::desk::MAX_NAME`). */
export const MAX_NAME = 40;

// The faint background of each accent on a card (those of the mockups).
const TINTS: Record<string, string> = {
  "#7aa7e6": "#16202e",
  "#a99be8": "#1d1a2b",
  "#5fbfb0": "#14221f",
  "#d9a75a": "#1e1a12",
};

/** The faint background of the machine's accent (group icons, default environment). */
export function tint(color: string): string {
  return TINTS[color.toLowerCase()] ?? `color-mix(in srgb, ${color} 9%, #15161a)`;
}

/** A decimal without ".0" (`17.1`, `24`). */
function one(v: number): string {
  const r = Math.round(v * 10) / 10;
  return Number.isInteger(r) ? String(r) : r.toFixed(1);
}

/** Bytes in GB, as the system shows them (base 1024): `17.1`, `24`, `412`. */
export function gb(bytes: number): string {
  const v = bytes / GIB;
  return v >= 100 ? String(Math.round(v)) : one(v);
}

/** "412 GB free of 1.8 TB". */
export function diskText(free: number, total: number): string {
  const t = total / GIB;
  const totalText = t >= 1000 ? `${one(t / 1024)} TB` : `${gb(total)} GB`;
  return `${gb(free)} GB free of ${totalText}`;
}

/** Percentage from 0 to 100 (clamped). */
export function pct(part: number, total: number): number {
  if (!(total > 0)) return 0;
  return Math.max(0, Math.min(100, (part / total) * 100));
}

/** The processor model without brands or padding: "Ryzen 7", "M5", "Core i7-13700K". */
export function shortCpu(model: string): string {
  return model
    .replace(/\((R|TM)\)/gi, "")
    .replace(/^(AMD|Intel|Apple)\s+/i, "")
    .replace(/\s+\d+-Core Processor$/i, "")
    .replace(/\s+(Processor|CPU)\b.*$/i, "")
    .replace(/\s+@.*$/, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** The GPU name without the brand: "RTX 5080", "M5". */
export function shortGpu(name: string): string {
  return name
    .replace(/\((R|TM)\)/gi, "")
    .replace(/^(NVIDIA|AMD|Apple|Intel)\s+/i, "")
    .replace(/^GeForce\s+/i, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** Under "CPU": short model and logical cores ("Ryzen 7 · 16 threads", "M5 · 12 cores"). */
export function cpuSub(hw: HardwareView | null, os: MachineView["os"]): string {
  if (!hw) return "";
  const cores = hw.cores > 0 ? `${hw.cores} ${os === "windows" ? "threads" : "cores"}` : "";
  return [hw.cpu ? shortCpu(hw.cpu) : "", cores].filter(Boolean).join(" · ");
}

/** The GPU shares the RAM (Apple): it has no VRAM of its own. */
export function sharedMemory(hw: HardwareView | null): boolean {
  return !!hw?.gpu && hw.gpu.vram === null;
}

/** Under "RAM": the total, or "unified" if the GPU shares it. */
export function ramSub(hw: HardwareView | null, live: LiveView | null): string {
  if (sharedMemory(hw)) return "unified";
  const total = hw?.memory || live?.ram.total || 0;
  return total > 0 ? `${gb(total)} GB` : "";
}

/** Under "GPU": "RTX 5080" or "M5 integrated". */
export function gpuSub(hw: HardwareView | null): string {
  if (!hw?.gpu) return "";
  const n = shortGpu(hw.gpu.name);
  return sharedMemory(hw) ? `${n} integrated` : n;
}

/** The VRAM passes the notice threshold. */
export function vramWarn(live: LiveView | null): boolean {
  const g = live?.gpu;
  if (!g || g.vramUsed === null || !g.vramTotal) return false;
  return g.vramUsed / g.vramTotal >= VRAM_WARN;
}

const POWER_TEXT: Record<Power, string> = { on: "online", off: "off", booting: "powering on", lost: "offline" };

/** The line under the name: the other one says whether it is online; this one, its processor. */
export function headerSub(m: Pick<MachineView, "here" | "system" | "power" | "hardware">): string {
  const second = m.here ? (m.hardware?.cpu ?? "") : POWER_TEXT[m.power];
  return [m.system, second].filter(Boolean).join(" · ");
}

/** The name shown for an environment: WSL distros say they are one. */
export function envLabel(e: Pick<EnvView, "kind" | "name">): string {
  return e.kind === "wsl" ? `WSL · ${e.name}` : e.name;
}

/**
 * The environment where remote work runs if nobody chooses (the highlighted one) goes first; the rest in the
 * agent's order. The default WSL distro is preferred over the system shell.
 */
export function orderEnvs(envs: EnvView[]): { main: EnvView | null; rest: EnvView[] } {
  const main = envs.find((e) => e.default && e.kind === "wsl") ?? envs.find((e) => e.default && e.kind === "shell") ?? envs.find((e) => e.default) ?? null;
  return { main, rest: envs.filter((e) => e !== main) };
}

// What stops working without each macOS permission (the "Permissions" row, `F-MachineMac`).
const PERMISSION_TEXT: Record<Permission, string> = {
  input_monitoring: "“Input Monitoring” is missing: it cannot send the keyboard. It is checked automatically.",
  accessibility: "“Accessibility” is missing: it cannot move the mouse or type here. It is checked automatically.",
  local_network: "“Local Network” is missing: it cannot find the other machine. It is checked automatically.",
};

// The most noticeable first: without a keyboard you cannot work.
const PERMISSION_ORDER: Permission[] = ["input_monitoring", "accessibility", "local_network"];

/** The missing permission that the "Permissions" row shows first, with its text. */
export function missingPermission(missing: Permission[]): { id: Permission; text: string } | null {
  const id = PERMISSION_ORDER.find((p) => missing.includes(p));
  return id ? { id, text: PERMISSION_TEXT[id] } : null;
}

export const MODULE_TEXT: Record<ModuleState, string> = { active: "active", off: "off", unavailable: "unavailable" };

// ---- Trend lines --------------------------------------------------------------------------

/** `points` of a polyline of `width`×`height` with values 0–100 (bottom is 0). */
export function trendPoints(values: number[], width = 120, height = 34): string {
  if (values.length === 0) return "";
  const step = values.length > 1 ? width / (values.length - 1) : 0;
  const r = (n: number) => Math.round(n * 100) / 100;
  return values.map((v, i) => `${r(i * step)},${r(height - (Math.max(0, Math.min(100, v)) * height) / 100)}`).join(" ");
}

/** The area under the line (the faint CPU fill). */
export function trendArea(values: number[], width = 120, height = 34): string {
  const pts = trendPoints(values, width, height);
  if (!pts) return "";
  return `M${pts.split(" ").join(" L")} L${width} ${height} L0 ${height}Z`;
}

// ---- Agent samples → live view -------------------------------------------------------------

const push = (h: number[], v: number) => [...h, v].slice(-HISTORY);

/** The live view with a new sample (the history of each line advances one point). */
export function nextLive(prev: LiveView | null, s: ResourceSample): LiveView {
  const cpu = s.cpu.load / 10;
  const g = s.gpus[0];
  const gpuPct = g?.load != null ? g.load / 10 : null;
  return {
    cpu: { percent: cpu, temp: s.cpu.temperature, history: push(prev?.cpu.history ?? [], cpu) },
    ram: { used: s.memory.used, total: s.memory.total, history: push(prev?.ram.history ?? [], pct(s.memory.used, s.memory.total)) },
    gpu: g
      ? {
          percent: gpuPct,
          vramUsed: g.vram_used,
          vramTotal: g.vram_total,
          temp: g.temperature,
          fans: null,
          history: gpuPct === null ? (prev?.gpu?.history ?? []) : push(prev?.gpu?.history ?? [], gpuPct),
        }
      : null,
    disk: s.disk,
    battery: prev?.battery ?? null,
  };
}

/** The summary that the Desk and the tray show, from the live view. */
export function summaryOf(live: LiveView): Resources {
  const ram = pct(live.ram.used, live.ram.total);
  const g = live.gpu;
  let gpu: Resources["gpu"] = null;
  let gpuLabel: Resources["gpuLabel"] = "GPU";
  if (g && g.vramUsed !== null && g.vramTotal) {
    gpu = { percent: pct(g.vramUsed, g.vramTotal), label: `${gb(g.vramUsed)}/${gb(g.vramTotal)} GB` };
    gpuLabel = "VRAM";
  } else if (g && g.percent !== null) {
    gpu = { percent: g.percent, label: `${Math.round(g.percent)} %` };
  }
  return {
    cpu: { percent: live.cpu.percent, label: `${Math.round(live.cpu.percent)} %` },
    ram: { percent: ram, label: `${gb(live.ram.used)}/${gb(live.ram.total)} GB` },
    gpu,
    gpuLabel,
    history: live.cpu.history,
  };
}

/** What the page can edit of the name: trimmed and within the desk's length. */
export function checkName(raw: string): { ok: true; name: string } | { ok: false; reason: string } {
  const name = raw.trim();
  if (!name) return { ok: false, reason: "The name cannot be empty" };
  if ([...name].length > MAX_NAME) return { ok: false, reason: `The name cannot exceed ${MAX_NAME} characters` };
  return { ok: true, name };
}
