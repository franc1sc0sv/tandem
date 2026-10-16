// Atajos globales vistos desde la ventana: leer una combinación del teclado, mostrarla y
// decidir cuáles atiende la ventana misma. La forma canónica es la de
// `tandem_ui::shortcuts` (`Ctrl+Alt+Shift+Super+<KeyboardEvent.code>`); las pruebas de los
// dos lados la fijan.

import type { Bindings, ShortcutAction, ShortcutsView } from "./model";
import type { ModeKey } from "./scene";

export const ACTIONS: ShortcutAction[] = ["solo_mac", "shared", "solo_pc", "toggle_window"];

export const ACTION_LABEL: Record<ShortcutAction, string> = {
  solo_mac: "Solo Mac",
  shared: "Compartido",
  solo_pc: "Solo Windows",
  toggle_window: "Mostrar u ocultar Tandem",
};

export const ACTION_OF_MODE: Record<ModeKey, ShortcutAction> = { mac: "solo_mac", shared: "shared", pc: "solo_pc" };

/** Los de siempre (`Shortcuts::default`). */
export const DEFAULT_BINDINGS: Bindings = {
  solo_mac: "Ctrl+Alt+Digit1",
  shared: "Ctrl+Alt+Digit2",
  solo_pc: "Ctrl+Alt+Digit3",
  toggle_window: "Ctrl+Alt+Digit0",
};

const NAMED_KEYS = [
  "Backquote", "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote", "Comma",
  "Period", "Slash", "Space", "Enter", "Tab", "Escape", "Backspace", "Delete", "Insert", "Home", "End",
  "PageUp", "PageDown", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
];

/** Las mismas teclas que acepta el núcleo. */
export function isKey(code: string): boolean {
  return NAMED_KEYS.includes(code) || /^(Key[A-Z]|Digit[0-9]|Numpad[0-9]|F([1-9]|1[0-9]|2[0-4]))$/.test(code);
}

const MODIFIER_CODES = /^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/;

type KeyLike = Pick<KeyboardEvent, "ctrlKey" | "altKey" | "shiftKey" | "metaKey" | "code">;

export type Combo =
  /** Solo modificadores por ahora: todavía se está escribiendo. */
  | { kind: "partial" }
  | { kind: "invalid"; reason: string }
  | { kind: "ok"; accel: string };

/** Lee una combinación de un `keydown`, por código físico (en macOS ⌥ cambia la tecla). */
export function readCombo(e: KeyLike): Combo {
  if (MODIFIER_CODES.test(e.code)) return { kind: "partial" };
  if (!isKey(e.code)) return { kind: "invalid", reason: "esa tecla no se puede usar en un atajo global" };
  if (!e.ctrlKey && !e.altKey && !e.metaKey) {
    return { kind: "invalid", reason: "hace falta Ctrl, Alt o ⌘/Win: solo con Mayúsculas se comería lo que escribes" };
  }
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean);
  return { kind: "ok", accel: [...mods, e.code].join("+") };
}

const KEY_TEXT: Record<string, string> = {
  Backquote: "`", Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\", Semicolon: ";",
  Quote: "'", Comma: ",", Period: ".", Slash: "/", Space: "Espacio", Enter: "↩", Tab: "⇥", Escape: "Esc",
  Backspace: "⌫", Delete: "Supr", Insert: "Insert", Home: "Inicio", End: "Fin", PageUp: "RePág", PageDown: "AvPág",
  ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
};

function keyText(code: string): string {
  if (code in KEY_TEXT) return KEY_TEXT[code];
  const m = /^(?:Key|Digit)(.)$/.exec(code);
  if (m) return m[1];
  const n = /^Numpad(\d)$/.exec(code);
  if (n) return `Num ${n[1]}`;
  return code;
}

/** Cómo se ve un atajo: `⌃⌥1` en macOS, `Ctrl+Alt+1` en Windows. */
export function formatAccel(accel: string | null | undefined, mac: boolean): string {
  if (!accel) return "";
  const parts = accel.split("+");
  const key = keyText(parts.pop() ?? "");
  const has = (m: string) => parts.includes(m);
  if (mac) {
    return `${has("Ctrl") ? "⌃" : ""}${has("Alt") ? "⌥" : ""}${has("Shift") ? "⇧" : ""}${has("Super") ? "⌘" : ""}${key}`;
  }
  const names = [has("Ctrl") && "Ctrl", has("Alt") && "Alt", has("Shift") && "Mayús", has("Super") && "Win"].filter(Boolean);
  return [...names, key].join("+");
}

/** Para `aria-keyshortcuts`. */
export function ariaKeys(accel: string | null | undefined): string {
  if (!accel) return "";
  const names: Record<string, string> = { Ctrl: "Control", Alt: "Alt", Shift: "Shift", Super: "Meta" };
  const parts = accel.split("+");
  const key = parts.pop() ?? "";
  const k = /^(?:Key|Digit)(.)$/.exec(key)?.[1] ?? key;
  return [...parts.map((p) => names[p] ?? p), k].join("+");
}

export function isMacPlatform(ua: string = navigator.userAgent): boolean {
  return /Mac/.test(ua);
}

/**
 * La acción que la ventana debe atender ella misma con esta tecla: solo las que el sistema
 * no dejó registrar como globales (si no, el atajo global ya la atiende y correría dos veces).
 */
export function localAction(e: KeyLike, view: ShortcutsView): ShortcutAction | null {
  const c = readCombo(e);
  if (c.kind !== "ok") return null;
  const hit = ACTIONS.find((a) => view.bindings[a] === c.accel);
  return hit && hit in view.failed ? hit : null;
}

/** Atajos repetidos en lo que se está editando (el núcleo también lo rechaza). */
export function duplicateOf(bindings: Bindings, action: ShortcutAction, accel: string): ShortcutAction | null {
  return ACTIONS.find((a) => a !== action && bindings[a] === accel) ?? null;
}
