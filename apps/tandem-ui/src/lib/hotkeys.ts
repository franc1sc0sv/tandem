// Settings → Shortcuts: the rows (window, lock, palette and one per scene), which combinations
// each accepts and the clashes between them. Pure: the screen only draws and saves.
//
// Decision (DECISIONS): ⌘K / Ctrl+K belongs to the window only. As a global shortcut it would eat Ctrl+K
// from any other app, and the palette only makes sense with Tandem in front.

import { codesOfAccel } from "./scenes";
import { isKey, readCombo, type Combo } from "./shortcuts";

export type RowId = "toggle_window" | "lock" | "palette" | `scene:${string}`;

export interface ShortcutRow {
  id: RowId;
  label: string;
  /** Canonical combination (`Ctrl+Alt+Digit0`, `ScrollLock`); empty without a shortcut. */
  accel: string;
  editable: boolean;
}

/** Keys that lock on their own, without a modifier: none of them is typed into text. */
const LONE_KEYS = /^(ScrollLock|Pause|PrintScreen|F([1-9]|1[0-9]|2[0-4]))$/;

const CTRL = /^Control(Left|Right)?$/;
const ALT = /^Alt(Left|Right)?$/;
const SHIFT = /^Shift(Left|Right)?$/;
const META = /^(Meta|OS)(Left|Right)?$/;

function isMainKey(code: string): boolean {
  return LONE_KEYS.test(code) || isKey(code);
}

/** The canonical combination of the keys of a desk shortcut (lock or scene). */
export function canon(codes: string[]): string {
  const key = codes.find(isMainKey);
  if (!key) return "";
  const mods = [codes.some((c) => CTRL.test(c)) && "Ctrl", codes.some((c) => ALT.test(c)) && "Alt", codes.some((c) => SHIFT.test(c)) && "Shift", codes.some((c) => META.test(c)) && "Super"].filter(Boolean);
  return [...mods, key].join("+");
}

/** What is read from the keyboard for the lock: like a global shortcut, or a key that works alone. */
export function readLockCombo(e: Parameters<typeof readCombo>[0]): Combo {
  if (LONE_KEYS.test(e.code) && !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey) return { kind: "ok", accel: e.code };
  return readCombo(e);
}

/** What the desk stores for a canonical combination. */
export function codesOf(accel: string): string[] {
  return accel ? codesOfAccel(accel) : [];
}

export interface Inputs {
  toggle: string | null;
  lock: string[];
  scenes: { id: string; name: string; shortcut: string[] }[];
  /** ⌘K on macOS, Ctrl+K elsewhere, in canonical form. */
  palette: string;
}

export function rowsOf(i: Inputs): ShortcutRow[] {
  return [
    { id: "toggle_window", label: "Show or hide Tandem", accel: i.toggle ?? "", editable: true },
    { id: "lock", label: "Lock the keyboard on this machine", accel: canon(i.lock), editable: true },
    { id: "palette", label: "Command palette", accel: i.palette, editable: false },
    ...i.scenes.map((s): ShortcutRow => ({ id: `scene:${s.id}`, label: `Scene “${s.name}”`, accel: canon(s.shortcut), editable: true })),
  ];
}

/** Who already uses this combination (another row), to reject it with its name. */
export function clashOf(rows: ShortcutRow[], id: RowId, accel: string): ShortcutRow | null {
  if (!accel) return null;
  return rows.find((r) => r.id !== id && r.accel === accel) ?? null;
}
