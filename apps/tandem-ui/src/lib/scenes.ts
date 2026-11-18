// Escenas (D6): del puesto del agente a las tarjetas de Monitores, y el borrador del editor
// (`F-NuevaEscena`). Puro.

import { shortMonitor } from "./monitors";
import type { Machine, SceneJson } from "./model";
import { formatAccel, isKey } from "./shortcuts";
import type { MonitorUi, SceneView } from "./store/types";

const CTRL = /^Control(Left|Right)?$/;
const ALT = /^Alt(Left|Right)?$/;
const SHIFT = /^Shift(Left|Right)?$/;
const META = /^(Meta|OS)(Left|Right)?$/;

/** `["ControlLeft","AltLeft","Digit1"]` → `Ctrl+Alt+Digit1` (el formato de `formatAccel`). */
export function accelOfCodes(codes: string[]): string {
  const mods = [codes.some((c) => CTRL.test(c)) && "Ctrl", codes.some((c) => ALT.test(c)) && "Alt", codes.some((c) => SHIFT.test(c)) && "Shift", codes.some((c) => META.test(c)) && "Super"].filter(Boolean);
  const key = codes.find((c) => isKey(c));
  return key ? [...mods, key].join("+") : "";
}

/** `Ctrl+Alt+Digit4` → los códigos que guarda la escena. */
export function codesOfAccel(accel: string): string[] {
  const names: Record<string, string> = { Ctrl: "ControlLeft", Alt: "AltLeft", Shift: "ShiftLeft", Super: "MetaLeft" };
  return accel.split("+").map((p) => names[p] ?? p);
}

/** Cómo se ve el atajo de una escena en esta máquina (`⌃⌥1` o `Ctrl+Alt+1`); vacío sin atajo. */
export function shortcutText(codes: string[], mac: boolean): string {
  return formatAccel(accelOfCodes(codes), mac);
}

/** El mismo atajo (con cualquiera de las teclas de cada modificador). */
export function sameShortcut(a: string[], b: string[]): boolean {
  const x = accelOfCodes(a);
  return x !== "" && x === accelOfCodes(b);
}

export interface SceneContext {
  monitors: MonitorUi[];
  name: (m: Machine) => string;
  mac: boolean;
}

/** La tarjeta de una escena: `34" → PC-Escritorio`, su atajo y la máquina de la miniatura. */
export function sceneView(s: SceneJson, ctx: SceneContext): SceneView {
  const known = s.monitors.flatMap(([id, machine]) => {
    const m = ctx.monitors.find((x) => x.id === id);
    return m ? [{ m, machine }] : [];
  });
  const desc = known.length > 0 ? known.map(({ m, machine }) => `${shortMonitor(m.name)} → ${ctx.name(machine)}`).join(", ") : "Sin cambiar monitores";
  return {
    id: s.id,
    name: s.name,
    key: shortcutText(s.shortcut, ctx.mac),
    desc,
    shows: s.monitors[0]?.[1] ?? null,
    monitors: s.monitors.map(([monitor, machine]) => ({ monitor, machine })),
    shortcut: s.shortcut,
    raw: s,
  };
}

// ---- El editor -----------------------------------------------------------------------------

/** Qué muestra un monitor en la escena: una máquina, o `keep` (sin cambiar). */
export type Pick = Machine | "keep";

export interface Draft {
  /** `null`: escena nueva. */
  id: string | null;
  name: string;
  picks: Record<string, Pick>;
  shortcut: string[];
  /** Lo que la pantalla no edita (acomodo, máquina requerida, ícono) se conserva. */
  base: SceneJson | null;
}

/** Igual que el núcleo (`MAX_SCENE_NAME`). */
export const MAX_NAME = 40;

/** Solo los compatibles entran a una escena (D11, DESIGN §5). */
export function compatible(monitors: MonitorUi[]): MonitorUi[] {
  return monitors.filter((m) => m.compat.ok);
}

/** Borrador de una escena nueva: cada monitor compatible, sin cambiar. */
export function newDraft(monitors: MonitorUi[]): Draft {
  return { id: null, name: "", picks: Object.fromEntries(compatible(monitors).map((m) => [m.id, "keep" as Pick])), shortcut: [], base: null };
}

export function draftOf(s: SceneView, monitors: MonitorUi[]): Draft {
  const picks: Record<string, Pick> = Object.fromEntries(compatible(monitors).map((m) => [m.id, "keep" as Pick]));
  // Un monitor de la escena que hoy no se ve se conserva: editar el nombre no lo borra.
  for (const { monitor, machine } of s.monitors) picks[monitor] = machine;
  return { id: s.id, name: s.name, picks, shortcut: s.shortcut, base: s.raw };
}

export type Problem = { field: "name" | "shortcut"; text: string };

/** Lo que impide guardar; `null` si se puede. `others` son las demás escenas (nombres y atajos ya tomados). */
export function problemOf(d: Draft, others: SceneView[]): Problem | null {
  const name = d.name.trim();
  if (name === "") return { field: "name", text: "Ponle un nombre a la escena" };
  if ([...name].length > MAX_NAME) return { field: "name", text: `El nombre no puede pasar de ${MAX_NAME} caracteres` };
  const clash = others.find((o) => sameShortcut(o.shortcut, d.shortcut));
  if (clash) return { field: "shortcut", text: `Ese atajo ya es el de «${clash.name}»` };
  return null;
}

/** La escena que se guarda: lo editado encima de lo que ya tenía. Una nueva va sin id: el agente se lo da. */
export function sceneOf(d: Draft): SceneJson {
  const monitors = Object.entries(d.picks).flatMap(([monitor, p]) => (p === "keep" ? [] : [[monitor, p] as [string, Machine]]));
  return { id: d.id ?? "", name: d.name.trim(), icon: d.base?.icon, monitors, layout: d.base?.layout ?? null, require_on: d.base?.require_on ?? null, shortcut: d.shortcut };
}
