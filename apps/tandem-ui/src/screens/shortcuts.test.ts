// T-129: Settings → Shortcuts. The pure logic of the rows and the clashes, and editing each shortcut
// (window, lock, scene) in demo mode and against the real agent (simulated IPC).
import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { buildCommands } from "../lib/commands";
import { canon, clashOf, codesOf, readLockCombo, rowsOf } from "../lib/hotkeys";
import { STORE_KEY } from "../lib/context";
import type { Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import Shell from "../shell/Shell.svelte";

describe("lib/shortcuts", () => {
  it("canon: modifiers in a fixed order and a single key", () => {
    expect(canon(["AltLeft", "ControlLeft", "Digit1"])).toBe("Ctrl+Alt+Digit1");
    expect(canon(["ControlRight", "KeyL"])).toBe("Ctrl+KeyL");
    expect(canon(["ScrollLock"])).toBe("ScrollLock");
    expect(canon([])).toBe("");
    expect(canon(["ControlLeft"])).toBe("");
  });

  it("codesOf and canon go back and forth", () => {
    for (const a of ["Ctrl+Alt+Digit1", "Ctrl+Shift+KeyJ", "Super+Digit4"]) expect(canon(codesOf(a))).toBe(a);
    expect(codesOf("")).toEqual([]);
  });

  it("the lock accepts ScrollLock alone, but a loose letter does not", () => {
    const k = (code: string, over = {}) => ({ code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...over });
    expect(readLockCombo(k("ScrollLock"))).toEqual({ kind: "ok", accel: "ScrollLock" });
    expect(readLockCombo(k("F13"))).toEqual({ kind: "ok", accel: "F13" });
    expect(readLockCombo(k("KeyL")).kind).toBe("invalid");
    expect(readLockCombo(k("KeyL", { ctrlKey: true }))).toEqual({ kind: "ok", accel: "Ctrl+KeyL" });
  });

  it("the rows follow the mockup order and the palette is not edited", () => {
    const rows = rowsOf({
      toggle: "Ctrl+Alt+Digit0",
      lock: ["ScrollLock"],
      palette: "Ctrl+KeyK",
      scenes: [{ id: "t", name: "Work", shortcut: ["ControlLeft", "AltLeft", "Digit1"] }],
    });
    expect(rows.map((r) => r.id)).toEqual(["toggle_window", "lock", "palette", "scene:t"]);
    expect(rows.find((r) => r.id === "palette")?.editable).toBe(false);
    expect(rows[3].label).toBe("Scene “Work”");
  });

  it("clashes: with the lock, with the window, with the palette and between scenes", () => {
    const rows = rowsOf({
      toggle: "Ctrl+Alt+Digit0",
      lock: ["ControlLeft", "KeyL"],
      palette: "Ctrl+KeyK",
      scenes: [
        { id: "a", name: "A", shortcut: ["ControlLeft", "AltLeft", "Digit1"] },
        { id: "b", name: "B", shortcut: [] },
      ],
    });
    expect(clashOf(rows, "scene:b", "Ctrl+KeyL")?.id).toBe("lock");
    expect(clashOf(rows, "scene:b", "Ctrl+Alt+Digit0")?.id).toBe("toggle_window");
    expect(clashOf(rows, "lock", "Ctrl+Alt+Digit1")?.id).toBe("scene:a");
    expect(clashOf(rows, "scene:b", "Ctrl+Alt+Digit1")?.id).toBe("scene:a");
    expect(clashOf(rows, "scene:b", "Ctrl+KeyK")?.id).toBe("palette");
    expect(clashOf(rows, "scene:a", "Ctrl+Alt+Digit1")).toBeNull();
    expect(clashOf(rows, "scene:b", "Ctrl+Alt+Digit7")).toBeNull();
  });
});

const chord = { ctrlKey: true, altKey: true };

async function open(store: Store) {
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  await userEvent.click(screen.getByRole("tab", { name: "Shortcuts" }));
  return r;
}

const comboOf = (label: RegExp | string) => screen.findByRole("button", { name: new RegExp(`Shortcut for “${label instanceof RegExp ? label.source : label}”`) });

describe("editing shortcuts in demo mode", () => {
  async function demo() {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/settings" });
    await store.start();
    await open(store);
    return store;
  }

  it("the mockup rows: window, lock, palette and one per scene", async () => {
    await demo();
    await comboOf("Show or hide Tandem");
    expect(await comboOf("Lock the keyboard on this machine")).toHaveTextContent("Scroll Lock");
    expect(await comboOf("Command palette")).toBeDisabled();
    expect(await comboOf("Scene “Work”")).toBeInTheDocument();
  });

  it("the lock is saved on recording it and accepts F13 alone", async () => {
    const store = await demo();
    const combo = await comboOf("Lock the keyboard on this machine");
    await userEvent.click(combo);
    await fireEvent.keyDown(combo, { key: "F13", code: "F13" });
    await vi.waitFor(() => expect(store.desk.lockShortcut).toEqual(["F13"]));
  });

  it("a scene saves its new shortcut", async () => {
    const store = await demo();
    const combo = await comboOf("Scene “Work”");
    await userEvent.click(combo);
    await fireEvent.keyDown(combo, { key: "7", code: "Digit7", ...chord });
    await vi.waitFor(() => expect(store.desk.scenes.find((s) => s.id === "trabajo")?.shortcut).toEqual(["ControlLeft", "AltLeft", "Digit7"]));
  });

  it("rejects another row's shortcut and saves nothing", async () => {
    const store = await demo();
    const before = store.desk.scenes.map((s) => s.shortcut);
    const combo = await comboOf("Scene “Work”");
    await userEvent.click(combo);
    // ⌃⌥2 is the second scene's.
    await fireEvent.keyDown(combo, { key: "2", code: "Digit2", ...chord });
    expect(screen.getByRole("alert")).toHaveTextContent(/Already used by “Scene “/);
    expect(store.desk.scenes.map((s) => s.shortcut)).toEqual(before);
    // ⌃⌥0 is the window's.
    await fireEvent.keyDown(combo, { key: "0", code: "Digit0", ...chord });
    expect(screen.getByRole("alert")).toHaveTextContent("Already used by “Show or hide Tandem”");
  });

  it("the lock cannot be a scene's", async () => {
    const store = await demo();
    const combo = await comboOf("Lock the keyboard on this machine");
    await userEvent.click(combo);
    await fireEvent.keyDown(combo, { key: "1", code: "Digit1", ...chord });
    expect(screen.getByRole("alert")).toHaveTextContent("Already used by “Scene “Work””");
    expect(store.desk.lockShortcut).toEqual(["ScrollLock"]);
  });
});

function connected(): Snapshot {
  return {
    kind: "connected",
    agent: "b",
    desk: {
      machines: { a: { os: "windows", name: "Workshop", color: "#6cc59a" }, b: null },
      screens: [],
      arrangement: { a_side: "up", align: "end" },
      scenes: [
        { id: "trabajo", name: "Work", monitors: [], shortcut: ["ControlLeft", "AltLeft", "Digit1"] },
        { id: "game", name: "Game", monitors: [], shortcut: [] },
      ],
      shortcuts: { lock: ["ScrollLock"] },
    },
    state: { on: { a: true, b: true } },
    monitors: [],
    peer: { kind: "paired", name: "Workshop", link: "wifi" },
    pairing: { kind: "idle" },
    input: { server: "b", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
    caps: {
      here: {
        machine: "b",
        name: "Studio",
        os: "windows",
        color: "#d9a75a",
        hub: null,
        input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: true },
        services: [],
      },
      peer: null,
    },
  } as unknown as Snapshot;
}

describe("editing shortcuts against the real agent (simulated IPC)", () => {
  async function live() {
    const act = vi.fn(async (_a: unknown): Promise<{ kind: "done" } | { kind: "rejected"; reason: string }> => ({ kind: "done" }));
    const bridge = {
      snapshot: async () => connected(),
      onSnapshot: async () => {},
      act,
      shortcuts: async () => ({ bindings: { toggle_window: "Ctrl+Alt+Digit0" }, failed: {}, warning: null, file: null }),
      setShortcuts: vi.fn(async (b: unknown) => ({ bindings: b, failed: {}, warning: null, file: null })),
    } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge), { hash: "#/settings" });
    await store.start();
    await open(store);
    return { act, bridge: bridge as unknown as { setShortcuts: ReturnType<typeof vi.fn> } };
  }

  it("the lock sends set_lock_shortcut with the codes", async () => {
    const { act } = await live();
    const combo = await comboOf("Lock the keyboard on this machine");
    await userEvent.click(combo);
    await fireEvent.keyDown(combo, { key: "l", code: "KeyL", ctrlKey: true });
    await vi.waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "set_lock_shortcut", lock: ["ControlLeft", "KeyL"] }));
  });

  it("\"Lock here\" from the palette sends lock_here (T-129)", async () => {
    const act = vi.fn(async (_a: unknown): Promise<{ kind: "done" }> => ({ kind: "done" }));
    const bridge = { snapshot: async () => connected(), onSnapshot: async () => {}, act } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge), { hash: "#/settings" });
    await store.start();
    const cmds = buildCommands(store.desk, store, false);
    await cmds.find((c) => c.id === "lock:on")?.run();
    await buildCommands({ ...store.desk, locked: true }, store, false).find((c) => c.id === "lock:off")?.run();
    expect(act).toHaveBeenCalledWith({ kind: "lock_here", on: true });
    expect(act).toHaveBeenCalledWith({ kind: "lock_here", on: false });
  });

  it("a scene sends edit_scene with its shortcut and the rest intact", async () => {
    const { act } = await live();
    const combo = await comboOf("Scene “Game”");
    await userEvent.click(combo);
    await fireEvent.keyDown(combo, { key: "3", code: "Digit3", ...chord });
    await vi.waitFor(() =>
      expect(act).toHaveBeenCalledWith({
        kind: "edit_scene",
        scene: expect.objectContaining({ id: "game", name: "Game", shortcut: ["ControlLeft", "AltLeft", "Digit3"] }),
      }),
    );
  });

  it("the window shortcut goes to ui.toml through setShortcuts", async () => {
    const { bridge } = await live();
    const combo = await comboOf("Show or hide Tandem");
    await userEvent.click(combo);
    await fireEvent.keyDown(combo, { key: "j", code: "KeyJ", ctrlKey: true });
    await vi.waitFor(() => expect(bridge.setShortcuts).toHaveBeenCalledWith({ toggle_window: "Ctrl+KeyJ" }));
  });

  it("a clash is rejected without asking the agent for anything", async () => {
    const { act } = await live();
    const combo = await comboOf("Scene “Game”");
    await userEvent.click(combo);
    await fireEvent.keyDown(combo, { key: "1", code: "Digit1", ...chord });
    expect(screen.getByRole("alert")).toHaveTextContent("Already used by “Scene “Work””");
    await fireEvent.keyDown(combo, { key: "k", code: "KeyK", ctrlKey: true });
    expect(screen.getByRole("alert")).toHaveTextContent("Already used by “Command palette”");
    expect(act).not.toHaveBeenCalledWith(expect.objectContaining({ kind: "edit_scene" }));
  });

  it("if the agent rejects the change, the reason is shown", async () => {
    const { act } = await live();
    act.mockImplementation(async () => ({ kind: "rejected" as const, reason: "unknown key" }));
    const combo = await comboOf("Lock the keyboard on this machine");
    await userEvent.click(combo);
    await fireEvent.keyDown(combo, { key: "l", code: "KeyL", ctrlKey: true });
    expect(await screen.findByRole("alert")).toHaveTextContent("unknown key");
  });
});
