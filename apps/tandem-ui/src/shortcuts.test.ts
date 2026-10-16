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
} from "./shortcuts";

const key = (code: string, mods: Partial<KeyboardEvent> = {}) => ({
  ctrlKey: true,
  altKey: true,
  metaKey: false,
  shiftKey: false,
  code,
  ...mods,
});

describe("leer una combinación", () => {
  it("en la forma canónica del núcleo, por código físico", () => {
    expect(readCombo(key("Digit1"))).toEqual({ kind: "ok", accel: "Ctrl+Alt+Digit1" });
    expect(readCombo(key("KeyK", { ctrlKey: false, shiftKey: true, metaKey: true }))).toEqual({
      kind: "ok",
      accel: "Alt+Shift+Super+KeyK",
    });
    expect(readCombo(key("F12", { altKey: false }))).toEqual({ kind: "ok", accel: "Ctrl+F12" });
  });

  it("solo modificadores: todavía se está escribiendo", () => {
    for (const code of ["ControlLeft", "AltRight", "ShiftLeft", "MetaLeft", "OSRight"]) {
      expect(readCombo(key(code)).kind).toBe("partial");
    }
  });

  it("sin Ctrl, Alt ni ⌘ no sirve: se comería lo que escribes", () => {
    const c = readCombo(key("KeyA", { ctrlKey: false, altKey: false, shiftKey: true }));
    expect(c.kind).toBe("invalid");
  });

  it("teclas que el núcleo no registra", () => {
    expect(readCombo(key("IntlBackslash")).kind).toBe("invalid");
    expect(readCombo(key("MediaPlayPause")).kind).toBe("invalid");
  });

  it("las mismas teclas que acepta el núcleo", () => {
    for (const ok of ["KeyA", "KeyZ", "Digit0", "Numpad9", "F1", "F24", "ArrowLeft", "Space", "Backquote"]) {
      expect(isKey(ok), ok).toBe(true);
    }
    for (const bad of ["Keya", "Digit10", "F0", "F25", "F01", "Numpad10", "Fn", ""]) {
      expect(isKey(bad), bad).toBe(false);
    }
  });
});

describe("mostrar un atajo", () => {
  it("con símbolos en macOS", () => {
    expect(formatAccel("Ctrl+Alt+Digit1", true)).toBe("⌃⌥1");
    expect(formatAccel("Alt+Shift+Super+KeyK", true)).toBe("⌥⇧⌘K");
    expect(formatAccel("Ctrl+ArrowLeft", true)).toBe("⌃←");
  });

  it("con nombres en Windows", () => {
    expect(formatAccel("Ctrl+Alt+Digit1", false)).toBe("Ctrl+Alt+1");
    expect(formatAccel("Shift+Super+Numpad3", false)).toBe("Mayús+Win+Num 3");
    expect(formatAccel("Ctrl+F5", false)).toBe("Ctrl+F5");
  });

  it("sin atajo, nada", () => {
    expect(formatAccel(null, true)).toBe("");
    expect(ariaKeys(null)).toBe("");
  });

  it("para lectores de pantalla", () => {
    expect(ariaKeys("Ctrl+Alt+Digit1")).toBe("Control+Alt+1");
    expect(ariaKeys("Shift+Super+KeyK")).toBe("Shift+Meta+K");
  });

  it("reconoce macOS", () => {
    expect(isMacPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)")).toBe(true);
    expect(isMacPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
  });
});

describe("qué atiende la ventana", () => {
  const view = (failed: ShortcutsView["failed"]): ShortcutsView => ({
    bindings: { ...DEFAULT_BINDINGS },
    failed,
    warning: null,
    file: null,
  });

  it("solo los que el sistema no dejó registrar como globales", () => {
    expect(localAction(key("Digit2"), view({}))).toBeNull();
    expect(localAction(key("Digit2"), view({ shared: "ocupado" }))).toBe("shared");
    expect(localAction(key("Digit1"), view({ shared: "ocupado" }))).toBeNull();
  });

  it("con la combinación exacta", () => {
    const v = view({ solo_mac: "x" });
    expect(localAction(key("Digit1", { shiftKey: true }), v)).toBeNull();
    expect(localAction(key("Numpad1"), v)).toBeNull();
    expect(localAction(key("Digit1"), v)).toBe("solo_mac");
  });
});

describe("editar", () => {
  it("los de siempre son ⌃⌥1/2/3 y ⌃⌥0", () => {
    expect(ACTIONS.map((a) => formatAccel(DEFAULT_BINDINGS[a], true))).toEqual(["⌃⌥1", "⌃⌥2", "⌃⌥3", "⌃⌥0"]);
  });

  it("detecta un atajo repetido en otra acción", () => {
    expect(duplicateOf(DEFAULT_BINDINGS, "toggle_window", "Ctrl+Alt+Digit1")).toBe("solo_mac");
    expect(duplicateOf(DEFAULT_BINDINGS, "solo_mac", "Ctrl+Alt+Digit1")).toBeNull();
    expect(duplicateOf(DEFAULT_BINDINGS, "solo_mac", "Ctrl+Alt+KeyM")).toBeNull();
  });
});
