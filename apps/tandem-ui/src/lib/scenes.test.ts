import { describe, expect, it } from "vitest";

import { MAX_NAME, accelOfCodes, codesOfAccel, draftOf, newDraft, problemOf, sameShortcut, sceneOf, sceneView, shortcutText } from "./scenes";
import type { SceneJson } from "./model";
import type { MonitorUi } from "./store/types";

const mon = (id: string, ok = true): MonitorUi => ({
  id,
  name: id === "g5-34" ? 'Odyssey G5 34"' : 'Odyssey G5 32"',
  compat: ok ? { ok: true } : { ok: false, reason: "no" },
  connected: ok,
  owner: ok ? "a" : null,
  input: null,
  ports: {},
  shows: null,
  events: [],
});
const monitors = [mon("g5-34"), mon("g5-32", false)];
const scene = (over: Partial<SceneJson> = {}): SceneJson => ({
  id: "juego",
  name: "Game",
  monitors: [["g5-34", "b"]],
  layout: null,
  require_on: null,
  shortcut: ["ControlLeft", "AltLeft", "Digit4"],
  ...over,
});
const ctx = { monitors, name: (m: "a" | "b") => (m === "a" ? "MacBook Pro" : "PC-Desktop"), mac: true };

describe("scenes (D6)", () => {
  it("el shortcut va de los códigos de teclado al formato de `formatAccel` y de vuelta", () => {
    expect(accelOfCodes(["ControlRight", "AltLeft", "Digit4"])).toBe("Ctrl+Alt+Digit4");
    expect(accelOfCodes(["ControlLeft"])).toBe("");
    expect(codesOfAccel("Ctrl+Alt+Digit4")).toEqual(["ControlLeft", "AltLeft", "Digit4"]);
    expect(shortcutText(["ControlLeft", "AltLeft", "Digit4"], true)).toBe("⌃⌥4");
    expect(shortcutText([], true)).toBe("");
  });

  it("dos shortcuts son el mismo con cualquiera de las teclas de cada modificador; vacío nunca choca", () => {
    expect(sameShortcut(["ControlLeft", "Digit1"], ["ControlRight", "Digit1"])).toBe(true);
    expect(sameShortcut(["ControlLeft", "Digit1"], ["ControlLeft", "Digit2"])).toBe(false);
    expect(sameShortcut([], [])).toBe(false);
  });

  it("la tarjeta dice qué muestra cada monitor y qué machine pone la miniatura", () => {
    const v = sceneView(scene(), ctx);
    expect(v).toMatchObject({ id: "juego", key: "⌃⌥4", desc: '34" → PC-Desktop', shows: "b" });
    expect(sceneView(scene({ monitors: [] }), ctx)).toMatchObject({ desc: "No monitors changed", shows: null });
  });

  it("el borrador nuevo deja cada monitor compatible sin cambiar y no ofrece el no compatible", () => {
    const d = newDraft(monitors);
    expect(d).toMatchObject({ id: null, name: "", picks: { "g5-34": "keep" }, shortcut: [] });
    expect("g5-32" in d.picks).toBe(false);
  });

  it("editar conserva lo que la pantalla no edita y los monitors que hoy no se ven", () => {
    const base = scene({ icon: "bolt", monitors: [["viejo", "a"]], require_on: "b" });
    const d = draftOf(sceneView(base, ctx), monitors);
    expect(d.picks).toEqual({ "g5-34": "keep", viejo: "a" });
    expect(sceneOf({ ...d, name: " Nuevo " })).toMatchObject({ id: "juego", name: "Nuevo", icon: "bolt", require_on: "b", monitors: [["viejo", "a"]] });
  });

  it("una scene nueva se guarda sin id y sin los monitors «sin cambiar»", () => {
    const d = { ...newDraft(monitors), name: "Work" };
    d.picks["g5-34"] = "a";
    expect(sceneOf(d)).toMatchObject({ id: "", monitors: [["g5-34", "a"]], layout: null });
  });

  it("no deja guardar sin nombre, con nombre largo ni con el shortcut de otra scene", () => {
    const d = newDraft(monitors);
    expect(problemOf(d, [])).toEqual({ field: "name", text: "Give the scene a name" });
    expect(problemOf({ ...d, name: "x".repeat(MAX_NAME + 1) }, [])?.field).toBe("name");
    const other = sceneView(scene({ name: "Otra" }), ctx);
    expect(problemOf({ ...d, name: "Nueva", shortcut: ["ControlRight", "AltRight", "Digit4"] }, [other])).toEqual({ field: "shortcut", text: "That shortcut already belongs to “Otra”" });
    expect(problemOf({ ...d, name: "Nueva", shortcut: ["ControlLeft", "Digit9"] }, [other])).toBeNull();
  });
});
