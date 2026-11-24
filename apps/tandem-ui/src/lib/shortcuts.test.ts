import { describe, expect, it } from "vitest";

import type { ShortcutsView } from "./model";
import {
  ACTIONS,
  DEFAULT_BINDINGS,
  ariaKeys,
  duplicateOf,
  formatAccel,
  isKey,
  isMacPlatform,
  localAction,
  readCombo,
  windowCommand,
} from "./shortcuts";
const key = (code: string, mods: Partial<KeyboardEvent> = {}) => ({
  ctrlKey: true,
  altKey: true,
  metaKey: false,
  shiftKey: false,
  code,
  ...mods,
});

describe("reading a combination", () => {
  it("in the core's canonical form, by physical code", () => {
    expect(readCombo(key("Digit1"))).toEqual({ kind: "ok", accel: "Ctrl+Alt+Digit1" });
    expect(readCombo(key("KeyK", { ctrlKey: false, shiftKey: true, metaKey: true }))).toEqual({
      kind: "ok",
      accel: "Alt+Shift+Super+KeyK",
    });
    expect(readCombo(key("F12", { altKey: false }))).toEqual({ kind: "ok", accel: "Ctrl+F12" });
  });

  it("only modifiers: it is still being typed", () => {
    for (const code of ["ControlLeft", "AltRight", "ShiftLeft", "MetaLeft", "OSRight"]) {
      expect(readCombo(key(code)).kind).toBe("partial");
    }
  });

  it("without Ctrl, Alt or ⌘ it is no good: it would eat what you type", () => {
    const c = readCombo(key("KeyA", { ctrlKey: false, altKey: false, shiftKey: true }));
    expect(c.kind).toBe("invalid");
  });

  it("keys the core does not register", () => {
    expect(readCombo(key("IntlBackslash")).kind).toBe("invalid");
    expect(readCombo(key("MediaPlayPause")).kind).toBe("invalid");
  });

  it("the same keys the core accepts", () => {
    for (const ok of ["KeyA", "KeyZ", "Digit0", "Numpad9", "F1", "F24", "ArrowLeft", "Space", "Backquote"]) {
      expect(isKey(ok), ok).toBe(true);
    }
    for (const bad of ["Keya", "Digit10", "F0", "F25", "F01", "Numpad10", "Fn", ""]) {
      expect(isKey(bad), bad).toBe(false);
    }
  });
});

describe("showing a shortcut", () => {
  it("with symbols on macOS", () => {
    expect(formatAccel("Ctrl+Alt+Digit1", true)).toBe("⌃⌥1");
    expect(formatAccel("Alt+Shift+Super+KeyK", true)).toBe("⌥⇧⌘K");
    expect(formatAccel("Ctrl+ArrowLeft", true)).toBe("⌃←");
  });

  it("with names on Windows", () => {
    expect(formatAccel("Ctrl+Alt+Digit1", false)).toBe("Ctrl+Alt+1");
    expect(formatAccel("Shift+Super+Numpad3", false)).toBe("Shift+Win+Num 3");
    expect(formatAccel("Ctrl+F5", false)).toBe("Ctrl+F5");
  });

  it("without a shortcut, nothing", () => {
    expect(formatAccel(null, true)).toBe("");
    expect(ariaKeys(null)).toBe("");
  });

  it("for screen readers", () => {
    expect(ariaKeys("Ctrl+Alt+Digit1")).toBe("Control+Alt+1");
    expect(ariaKeys("Shift+Super+KeyK")).toBe("Shift+Meta+K");
  });

  it("recognizes macOS", () => {
    expect(isMacPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)")).toBe(true);
    expect(isMacPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
  });
});

describe("what the window handles", () => {
  const view = (failed: ShortcutsView["failed"]): ShortcutsView => ({
    bindings: { ...DEFAULT_BINDINGS },
    failed,
    warning: null,
    file: null,
  });

  it("only those the system did not let register as global, with the exact combination", () => {
    expect(localAction(key("Digit0"), view({}))).toBeNull();
    expect(localAction(key("Digit0"), view({ toggle_window: "busy" }))).toBe("toggle_window");
    expect(localAction(key("Digit0", { shiftKey: true }), view({ toggle_window: "x" }))).toBeNull();
    expect(localAction(key("Digit1"), view({ toggle_window: "x" }))).toBeNull();
  });

  it("showing or hiding hides the window, which is in front", () => {
    expect(windowCommand("toggle_window")).toEqual({ kind: "hide_main" });
  });
});

describe("editing", () => {
  it("the default is ⌃⌥0, and the mode shortcuts no longer exist", () => {
    expect(ACTIONS.map((a) => formatAccel(DEFAULT_BINDINGS[a], true))).toEqual(["⌃⌥0"]);
  });

  it("with a single action there are no duplicates", () => {
    expect(duplicateOf(DEFAULT_BINDINGS, "toggle_window", "Ctrl+Alt+Digit0")).toBeNull();
  });
});
