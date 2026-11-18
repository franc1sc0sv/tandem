// Ajustes → Atajos: las filas (ventana, bloqueo, paleta y una por escena), qué combinaciones
// admite cada una y los choques entre ellas. Puro: la pantalla solo dibuja y guarda.
//
// Decisión (DECISIONS): ⌘K / Ctrl+K es solo de la ventana. Como atajo global se comería Ctrl+K
// de cualquier otra app, y la paleta solo tiene sentido con Tandem al frente.

import { codesOfAccel } from "./scenes";
import { isKey, readCombo, type Combo } from "./shortcuts";

export type RowId = "toggle_window" | "lock" | "palette" | `scene:${string}`;

export interface AtajoRow {
  id: RowId;
  label: string;
  /** Combinación canónica (`Ctrl+Alt+Digit0`, `ScrollLock`); vacía sin atajo. */
  accel: string;
  editable: boolean;
}

/** Teclas que bloquean solas, sin modificador: ninguna se escribe en un texto. */
const LONE_KEYS = /^(ScrollLock|Pause|PrintScreen|F([1-9]|1[0-9]|2[0-4]))$/;

const CTRL = /^Control(Left|Right)?$/;
const ALT = /^Alt(Left|Right)?$/;
const SHIFT = /^Shift(Left|Right)?$/;
const META = /^(Meta|OS)(Left|Right)?$/;

function isMainKey(code: string): boolean {
  return LONE_KEYS.test(code) || isKey(code);
}

/** La combinación canónica de las teclas de un atajo del puesto (bloqueo o escena). */
export function canon(codes: string[]): string {
  const key = codes.find(isMainKey);
  if (!key) return "";
  const mods = [codes.some((c) => CTRL.test(c)) && "Ctrl", codes.some((c) => ALT.test(c)) && "Alt", codes.some((c) => SHIFT.test(c)) && "Shift", codes.some((c) => META.test(c)) && "Super"].filter(Boolean);
  return [...mods, key].join("+");
}

/** Lo que se lee del teclado para el bloqueo: como un atajo global, o una tecla que vale sola. */
export function readLockCombo(e: Parameters<typeof readCombo>[0]): Combo {
  if (LONE_KEYS.test(e.code) && !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey) return { kind: "ok", accel: e.code };
  return readCombo(e);
}

/** Lo que guarda el puesto para una combinación canónica. */
export function codesOf(accel: string): string[] {
  return accel ? codesOfAccel(accel) : [];
}

export interface Inputs {
  toggle: string | null;
  lock: string[];
  scenes: { id: string; name: string; shortcut: string[] }[];
  /** ⌘K en macOS, Ctrl+K en el resto, en forma canónica. */
  palette: string;
}

export function rowsOf(i: Inputs): AtajoRow[] {
  return [
    { id: "toggle_window", label: "Show or hide Tandem", accel: i.toggle ?? "", editable: true },
    { id: "lock", label: "Lock the keyboard on this machine", accel: canon(i.lock), editable: true },
    { id: "palette", label: "Command palette", accel: i.palette, editable: false },
    ...i.scenes.map((s): AtajoRow => ({ id: `scene:${s.id}`, label: `Escena «${s.name}»`, accel: canon(s.shortcut), editable: true })),
  ];
}

/** Quién ya usa esta combinación (otra fila), para rechazarla con su nombre. */
export function clashOf(rows: AtajoRow[], id: RowId, accel: string): AtajoRow | null {
  if (!accel) return null;
  return rows.find((r) => r.id !== id && r.accel === accel) ?? null;
}
