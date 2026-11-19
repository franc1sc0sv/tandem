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
  id: "game",
  name: "Game",
  monitors: [["g5-34", "b"]],
  layout: null,
  require_on: null,
  shortcut: ["ControlLeft", "AltLeft", "Digit4"],
  ...over,
});
const ctx = { monitors, name: (m: "a" | "b") => (m === "a" ? "MacBook Pro" : "PC-Desktop"), mac: true };

describe("scenes (D6)", () => {
  it("the shortcut goes from keyboard codes to the `formatAccel` format and back", () => {
    expect(accelOfCodes(["ControlRight", "AltLeft", "Digit4"])).toBe("Ctrl+Alt+Digit4");
    expect(accelOfCodes(["ControlLeft"])).toBe("");
    expect(codesOfAccel("Ctrl+Alt+Digit4")).toEqual(["ControlLeft", "AltLeft", "Digit4"]);
    expect(shortcutText(["ControlLeft", "AltLeft", "Digit4"], true)).toBe("⌃⌥4");
    expect(shortcutText([], true)).toBe("");
  });

  it("two shortcuts are the same with either of the keys of each modifier; empty never clashes", () => {
    expect(sameShortcut(["ControlLeft", "Digit1"], ["ControlRight", "Digit1"])).toBe(true);
    expect(sameShortcut(["ControlLeft", "Digit1"], ["ControlLeft", "Digit2"])).toBe(false);
    expect(sameShortcut([], [])).toBe(false);
  });

  it("the card says what each monitor shows and which machine sets the thumbnail", () => {
    const v = sceneView(scene(), ctx);
    expect(v).toMatchObject({ id: "game", key: "⌃⌥4", desc: '34" → PC-Desktop', shows: "b" });
    expect(sceneView(scene({ monitors: [] }), ctx)).toMatchObject({ desc: "No monitors changed", shows: null });
  });

  it("the new draft leaves each compatible monitor unchanged and does not offer the incompatible one", () => {
    const d = newDraft(monitors);
    expect(d).toMatchObject({ id: null, name: "", picks: { "g5-34": "keep" }, shortcut: [] });
    expect("g5-32" in d.picks).toBe(false);
  });

  it("editing preserves what the screen does not edit and the monitors that are not visible today", () => {
    const base = scene({ icon: "bolt", monitors: [["old", "a"]], require_on: "b" });
    const d = draftOf(sceneView(base, ctx), monitors);
    expect(d.picks).toEqual({ "g5-34": "keep", old: "a" });
    expect(sceneOf({ ...d, name: " New " })).toMatchObject({ id: "game", name: "New", icon: "bolt", require_on: "b", monitors: [["old", "a"]] });
  });

  it("a new scene is saved without an id and without the \"unchanged\" monitors", () => {
    const d = { ...newDraft(monitors), name: "Work" };
    d.picks["g5-34"] = "a";
    expect(sceneOf(d)).toMatchObject({ id: "", monitors: [["g5-34", "a"]], layout: null });
  });

  it("does not allow saving without a name, with a long name or with another scene's shortcut", () => {
    const d = newDraft(monitors);
    expect(problemOf(d, [])).toEqual({ field: "name", text: "Give the scene a name" });
    expect(problemOf({ ...d, name: "x".repeat(MAX_NAME + 1) }, [])?.field).toBe("name");
    const other = sceneView(scene({ name: "Other" }), ctx);
    expect(problemOf({ ...d, name: "New", shortcut: ["ControlRight", "AltRight", "Digit4"] }, [other])).toEqual({ field: "shortcut", text: "That shortcut already belongs to “Other”" });
    expect(problemOf({ ...d, name: "New", shortcut: ["ControlLeft", "Digit9"] }, [other])).toBeNull();
  });
});
