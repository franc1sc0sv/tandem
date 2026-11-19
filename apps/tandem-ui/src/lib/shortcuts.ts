// Global shortcuts as seen from the window: reading a combination from the keyboard, showing it and
// deciding which ones the window itself handles. The canonical form is that of
// `tandem_ui::shortcuts` (`Ctrl+Alt+Shift+Super+<KeyboardEvent.code>`); the tests on
// both sides pin it.

import type { Bindings, ShortcutAction, ShortcutsView } from "./model";

// The mode shortcuts left along with the modes (DESIGN §8): the window one remains.
export const ACTIONS: ShortcutAction[] = ["toggle_window"];

export const ACTION_LABEL: Record<ShortcutAction, string> = {
  toggle_window: "Show or hide Tandem",
};

/** The usual ones (`Shortcuts::default`). */
export const DEFAULT_BINDINGS: Bindings = {
  toggle_window: "Ctrl+Alt+Digit0",
};

const NAMED_KEYS = [
  "Backquote", "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote", "Comma",
  "Period", "Slash", "Space", "Enter", "Tab", "Escape", "Backspace", "Delete", "Insert", "Home", "End",
  "PageUp", "PageDown", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
];

/** The same keys the core accepts. */
export function isKey(code: string): boolean {
  return NAMED_KEYS.includes(code) || /^(Key[A-Z]|Digit[0-9]|Numpad[0-9]|F([1-9]|1[0-9]|2[0-4]))$/.test(code);
}

const MODIFIER_CODES = /^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/;

type KeyLike = Pick<KeyboardEvent, "ctrlKey" | "altKey" | "shiftKey" | "metaKey" | "code">;

export type Combo =
  /** Only modifiers so far: it is still being typed. */
  | { kind: "partial" }
  | { kind: "invalid"; reason: string }
  | { kind: "ok"; accel: string };

/** Reads a combination from a `keydown`, by physical code (on macOS ⌥ changes the key). */
export function readCombo(e: KeyLike): Combo {
  if (MODIFIER_CODES.test(e.code)) return { kind: "partial" };
  if (!isKey(e.code)) return { kind: "invalid", reason: "that key cannot be used in a global shortcut" };
  if (!e.ctrlKey && !e.altKey && !e.metaKey) {
    return { kind: "invalid", reason: "Ctrl, Alt or ⌘/Win is needed: with Shift alone it would swallow what you type" };
  }
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean);
  return { kind: "ok", accel: [...mods, e.code].join("+") };
}

const KEY_TEXT: Record<string, string> = {
  Backquote: "`", Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\", Semicolon: ";",
  Quote: "'", Comma: ",", Period: ".", Slash: "/", Space: "Space", Enter: "↩", Tab: "⇥", Escape: "Esc",
  Backspace: "⌫", Delete: "Del", Insert: "Insert", Home: "Start", End: "End", PageUp: "PgUp", PageDown: "PgDn",
  ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
  ScrollLock: "Scroll Lock", Pause: "Pause", PrintScreen: "PrtScn",
};

function keyText(code: string): string {
  if (code in KEY_TEXT) return KEY_TEXT[code];
  const m = /^(?:Key|Digit)(.)$/.exec(code);
  if (m) return m[1];
  const n = /^Numpad(\d)$/.exec(code);
  if (n) return `Num ${n[1]}`;
  return code;
}

/** How a shortcut looks: `⌃⌥1` on macOS, `Ctrl+Alt+1` on Windows. */
export function formatAccel(accel: string | null | undefined, mac: boolean): string {
  if (!accel) return "";
  const parts = accel.split("+");
  const key = keyText(parts.pop() ?? "");
  const has = (m: string) => parts.includes(m);
  if (mac) {
    return `${has("Ctrl") ? "⌃" : ""}${has("Alt") ? "⌥" : ""}${has("Shift") ? "⇧" : ""}${has("Super") ? "⌘" : ""}${key}`;
  }
  const names = [has("Ctrl") && "Ctrl", has("Alt") && "Alt", has("Shift") && "Shift", has("Super") && "Win"].filter(Boolean);
  return [...names, key].join("+");
}

/** For `aria-keyshortcuts`. */
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
 * The action the window must handle itself with this key: only those the system
 * did not let register as global (otherwise the global shortcut already handles it and it would run twice).
 */
export function localAction(e: KeyLike, view: ShortcutsView): ShortcutAction | null {
  const c = readCombo(e);
  if (c.kind !== "ok") return null;
  const hit = ACTIONS.find((a) => view.bindings[a] === c.accel);
  return hit && hit in view.failed ? hit : null;
}

export type WindowCommand = { kind: "hide_main" };

/**
 * What the main window does with an action it handles itself. If it receives the key
 * it is because it is in front: show or hide, then, hides it.
 */
export function windowCommand(action: ShortcutAction): WindowCommand {
  switch (action) {
    case "toggle_window":
      return { kind: "hide_main" };
  }
}

/** Duplicate shortcuts in what is being edited (the core also rejects it). */
export function duplicateOf(bindings: Bindings, action: ShortcutAction, accel: string): ShortcutAction | null {
  return ACTIONS.find((a) => a !== action && bindings[a] === accel) ?? null;
}
