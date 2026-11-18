// La página Máquina (T-115, `V3-Maquina`/`F-MaquinaMac`): textos, unidades y orden de lo que
// muestra, y cómo se arma la vista en vivo con las muestras del agente. Puro: se prueba sin
// pantalla.

import type { Permission, ResourceSample } from "./model";
import type { EnvView, HardwareView, LiveView, MachineView, ModuleState, Power, Resources } from "./store/types";

const GIB = 2 ** 30;

/** Muestras que guarda cada línea (una por segundo con el agente). */
export const HISTORY = 30;

/** Desde aquí la VRAM se da por casi llena (el aviso ámbar). */
export const VRAM_WARN = 0.85;

/** La paleta de colores de máquina (D17), con el nombre de cada uno. */
export const PALETTE: { color: string; name: string }[] = [
  { color: "#7aa7e6", name: "Azul" },
  { color: "#a99be8", name: "Violeta" },
  { color: "#5fbfb0", name: "Verde azulado" },
  { color: "#d9a75a", name: "Ámbar" },
];

/** El nombre largo máximo que acepta el puesto (`tandem_core::desk::MAX_NAME`). */
export const MAX_NAME = 40;

// El fondo tenue de cada acento sobre una tarjeta (los de los mockups).
const TINTS: Record<string, string> = {
  "#7aa7e6": "#16202e",
  "#a99be8": "#1d1a2b",
  "#5fbfb0": "#14221f",
  "#d9a75a": "#1e1a12",
};

/** El fondo tenue del acento de la máquina (íconos de grupo, entorno predeterminado). */
export function tint(color: string): string {
  return TINTS[color.toLowerCase()] ?? `color-mix(in srgb, ${color} 9%, #15161a)`;
}

/** Un decimal sin «.0» (`17.1`, `24`). */
function one(v: number): string {
  const r = Math.round(v * 10) / 10;
  return Number.isInteger(r) ? String(r) : r.toFixed(1);
}

/** Bytes en GB, como los muestra el sistema (base 1024): `17.1`, `24`, `412`. */
export function gb(bytes: number): string {
  const v = bytes / GIB;
  return v >= 100 ? String(Math.round(v)) : one(v);
}

/** «412 GB libres de 1.8 TB». */
export function diskText(free: number, total: number): string {
  const t = total / GIB;
  const totalText = t >= 1000 ? `${one(t / 1024)} TB` : `${gb(total)} GB`;
  return `${gb(free)} GB libres de ${totalText}`;
}

/** Porcentaje de 0 a 100 (acotado). */
export function pct(part: number, total: number): number {
  if (!(total > 0)) return 0;
  return Math.max(0, Math.min(100, (part / total) * 100));
}

/** El modelo del procesador sin marcas ni relleno: «Ryzen 7», «M5», «Core i7-13700K». */
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

/** El nombre de la GPU sin marca: «RTX 5080», «M5». */
export function shortGpu(name: string): string {
  return name
    .replace(/\((R|TM)\)/gi, "")
    .replace(/^(NVIDIA|AMD|Apple|Intel)\s+/i, "")
    .replace(/^GeForce\s+/i, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** Bajo «CPU»: modelo corto y núcleos lógicos («Ryzen 7 · 16 hilos», «M5 · 12 núcleos»). */
export function cpuSub(hw: HardwareView | null, os: MachineView["os"]): string {
  if (!hw) return "";
  const cores = hw.cores > 0 ? `${hw.cores} ${os === "windows" ? "hilos" : "núcleos"}` : "";
  return [hw.cpu ? shortCpu(hw.cpu) : "", cores].filter(Boolean).join(" · ");
}

/** La GPU comparte la RAM (Apple): no tiene VRAM propia. */
export function sharedMemory(hw: HardwareView | null): boolean {
  return !!hw?.gpu && hw.gpu.vram === null;
}

/** Bajo «RAM»: el total, o «unificada» si la GPU la comparte. */
export function ramSub(hw: HardwareView | null, live: LiveView | null): string {
  if (sharedMemory(hw)) return "unificada";
  const total = hw?.memory || live?.ram.total || 0;
  return total > 0 ? `${gb(total)} GB` : "";
}

/** Bajo «GPU»: «RTX 5080» o «M5 integrada». */
export function gpuSub(hw: HardwareView | null): string {
  if (!hw?.gpu) return "";
  const n = shortGpu(hw.gpu.name);
  return sharedMemory(hw) ? `${n} integrada` : n;
}

/** La VRAM pasa del umbral del aviso. */
export function vramWarn(live: LiveView | null): boolean {
  const g = live?.gpu;
  if (!g || g.vramUsed === null || !g.vramTotal) return false;
  return g.vramUsed / g.vramTotal >= VRAM_WARN;
}

const POWER_TEXT: Record<Power, string> = { on: "en línea", off: "apagada", booting: "encendiendo", lost: "sin conexión" };

/** La línea bajo el nombre: la otra dice si está en línea; esta, su procesador. */
export function headerSub(m: Pick<MachineView, "here" | "system" | "power" | "hardware">): string {
  const second = m.here ? (m.hardware?.cpu ?? "") : POWER_TEXT[m.power];
  return [m.system, second].filter(Boolean).join(" · ");
}

/** El nombre que se muestra de un entorno: las distros de WSL dicen que lo son. */
export function envLabel(e: Pick<EnvView, "kind" | "name">): string {
  return e.kind === "wsl" ? `WSL · ${e.name}` : e.name;
}

/**
 * El entorno donde corre lo remoto si nadie elige (el resaltado) va primero; los demás en el
 * orden del agente. Se prefiere la distro de WSL por omisión sobre el shell del sistema.
 */
export function orderEnvs(envs: EnvView[]): { main: EnvView | null; rest: EnvView[] } {
  const main = envs.find((e) => e.default && e.kind === "wsl") ?? envs.find((e) => e.default && e.kind === "shell") ?? envs.find((e) => e.default) ?? null;
  return { main, rest: envs.filter((e) => e !== main) };
}

// Qué deja de andar sin cada permiso de macOS (la fila «Permisos», `F-MaquinaMac`).
const PERMISSION_TEXT: Record<Permission, string> = {
  input_monitoring: "Falta «Monitorización de entrada»: no puede mandar el teclado. Se comprueba solo.",
  accessibility: "Falta «Accesibilidad»: no puede mover el mouse ni escribir aquí. Se comprueba solo.",
  local_network: "Falta «Red local»: no encuentra a la otra máquina. Se comprueba solo.",
};

// El que más se nota primero: sin teclado no se puede trabajar.
const PERMISSION_ORDER: Permission[] = ["input_monitoring", "accessibility", "local_network"];

/** El permiso que falta y que la fila «Permisos» muestra primero, con su texto. */
export function missingPermission(missing: Permission[]): { id: Permission; text: string } | null {
  const id = PERMISSION_ORDER.find((p) => missing.includes(p));
  return id ? { id, text: PERMISSION_TEXT[id] } : null;
}

export const MODULE_TEXT: Record<ModuleState, string> = { active: "activo", off: "apagado", unavailable: "no disponible" };

// ---- Líneas de tendencia ------------------------------------------------------------------

/** `points` de una polilínea de `width`×`height` con valores 0–100 (abajo es 0). */
export function trendPoints(values: number[], width = 120, height = 34): string {
  if (values.length === 0) return "";
  const step = values.length > 1 ? width / (values.length - 1) : 0;
  const r = (n: number) => Math.round(n * 100) / 100;
  return values.map((v, i) => `${r(i * step)},${r(height - (Math.max(0, Math.min(100, v)) * height) / 100)}`).join(" ");
}

/** El área bajo la línea (el relleno tenue de la CPU). */
export function trendArea(values: number[], width = 120, height = 34): string {
  const pts = trendPoints(values, width, height);
  if (!pts) return "";
  return `M${pts.split(" ").join(" L")} L${width} ${height} L0 ${height}Z`;
}

// ---- Muestras del agente → vista en vivo ---------------------------------------------------

const push = (h: number[], v: number) => [...h, v].slice(-HISTORY);

/** La vista en vivo con una muestra nueva (la historia de cada línea avanza un punto). */
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

/** El resumen que muestran el Puesto y la bandeja, a partir de la vista en vivo. */
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

/** Lo que la página puede editar del nombre: recortado y dentro del largo del puesto. */
export function checkName(raw: string): { ok: true; name: string } | { ok: false; reason: string } {
  const name = raw.trim();
  if (!name) return { ok: false, reason: "El nombre no puede quedar vacío" };
  if ([...name].length > MAX_NAME) return { ok: false, reason: `El nombre no puede pasar de ${MAX_NAME} caracteres` };
  return { ok: true, name };
}
