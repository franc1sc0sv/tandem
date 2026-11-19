// D5: the journey of the "Changed from X · Undo" notice, end to end, through the agent (simulated
// IPC) and through demo mode; and the cap and closing of notices.
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../bridge";
import type { Notice, Snapshot } from "../model";
import { DemoSource, demoOptions } from "./demo";
import { LiveSource } from "./live";
import { Store, TOAST_MAX, TOAST_MS, TRAY_TOPICS } from "./store.svelte";

afterEach(() => {
  vi.useRealTimers();
});

const snap = {
  kind: "connected",
  agent: "a",
  desk: { machines: { a: { os: "macos", name: "Studio", color: "#7aa7e6" }, b: { os: "windows", name: "Workshop", color: "#6cc59a" } }, screens: [], arrangement: null, scenes: [] },
  state: { on: { a: true, b: true } },
  peer: { kind: "paired", name: "Workshop", link: "usb4" },
  pairing: { kind: "idle" },
  monitors: [],
  input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
  caps: {
    here: {
      machine: "a",
      name: "Studio",
      os: "macos",
      color: "#7aa7e6",
      hub: null,
      input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
      power: { wake_peer: true, wakeable: true, power_off: true },
      services: [],
    },
    peer: null,
  },
} as unknown as Snapshot;

function fake() {
  const act = vi.fn(async (_a: unknown) => ({ kind: "done" as const }));
  const listeners: ((n: Notice) => void)[] = [];
  const bridge = {
    snapshot: async () => snap,
    onSnapshot: async () => {},
    act,
    onNotice: async (cb: (n: Notice) => void) => {
      listeners.push(cb);
    },
  } as unknown as Bridge;
  return { bridge, act, emit: (n: Notice) => listeners.forEach((l) => l(n)) };
}

describe("desk change notice (D5) through the agent", () => {
  it("desk_changed -> notice -> \"Undo\" -> undo_desk with the previous version", async () => {
    const { bridge, act, emit } = fake();
    const store = new Store(new LiveSource(bridge));
    await store.start();
    emit({ kind: "desk_changed", by: "b", rev: 7, summary: "Arrangement", undo_to: 6 });
    expect(store.toasts).toHaveLength(1);
    expect(store.toasts[0]).toMatchObject({ text: "Changed from Workshop", body: "Arrangement", tone: "info", auto: false });
    expect(store.toasts[0].action?.label).toBe("Undo");

    store.toasts[0].action?.run();
    await vi.waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "undo_desk", to_rev: 6 }));
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Change undone", tone: "ok" }));
  });

  it("the notice with \"Undo\" does not go away by itself; the others do after 6 s", async () => {
    vi.useFakeTimers();
    const { bridge, emit } = fake();
    const store = new Store(new LiveSource(bridge));
    await store.start();
    emit({ kind: "desk_changed", by: "b", rev: 2, summary: "Shortcuts", undo_to: 1 });
    store.toast("Done");
    vi.advanceTimersByTime(TOAST_MS + 100);
    expect(store.toasts.map((t) => t.text)).toEqual(["Changed from Workshop"]);
    store.dismiss(store.toasts[0].id);
    expect(store.toasts).toEqual([]);
  });

  it("a held notice (`hold`, the sample stack of F-Notices) does not go away by itself", async () => {
    vi.useFakeTimers();
    const { bridge } = fake();
    const store = new Store(new LiveSource(bridge));
    await store.start();
    store.notify({ text: "Pinned", tone: "ok", hold: true });
    store.notify({ text: "Normal", tone: "ok" });
    expect(store.toasts.map((t) => t.auto)).toEqual([true, false]);
    vi.advanceTimersByTime(TOAST_MS + 100);
    expect(store.toasts.map((t) => t.text)).toEqual(["Pinned"]);
  });

  it("if the agent rejects the undo, it announces the reason", async () => {
    const { bridge, act, emit } = fake();
    act.mockImplementation(async (a: unknown) =>
      (a as { kind: string }).kind === "undo_desk" ? ({ kind: "rejected", reason: "that version is gone" } as never) : ({ kind: "done" } as never),
    );
    const store = new Store(new LiveSource(bridge));
    await store.start();
    emit({ kind: "desk_changed", by: "b", rev: 3, summary: "Arrangement", undo_to: 2 });
    store.toasts[0].action?.run();
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "that version is gone", tone: "bad" }));
  });

  it("the tray (without a main window) does not repeat the notice", async () => {
    const { bridge, emit } = fake();
    const store = new Store(new LiveSource(bridge), null, TRAY_TOPICS);
    await store.start();
    emit({ kind: "desk_changed", by: "b", rev: 3, summary: "Arrangement", undo_to: 2 });
    expect(store.toasts).toEqual([]);
  });

  it("the stack keeps at most TOAST_MAX notices and the newest goes on top", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    for (let i = 0; i < TOAST_MAX + 3; i++) store.toast(`notice ${i}`);
    expect(store.toasts).toHaveLength(TOAST_MAX);
    expect(store.toasts[0].text).toBe(`notice ${TOAST_MAX + 2}`);
  });
});

describe("desk change notice (D5) in demo mode", () => {
  it("the other machine changes the desk, announces it and \"Undo\" restores the previous state", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    const before = store.desk.ready ? store.desk.prefs : null;
    expect(before).not.toBeNull();

    src.peerEdit("Clipboard off");
    expect(store.desk.ready && store.desk.prefs.clipboard).toBe(!before?.clipboard);
    expect(store.toasts[0]).toMatchObject({ body: "Clipboard off", auto: false });
    expect(store.toasts[0].text).toMatch(/^Changed from /);

    store.toasts[0].action?.run();
    await vi.waitFor(() => expect(store.desk.ready && store.desk.prefs).toEqual(before));
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Change undone", tone: "ok" }));
  });

  it("?peeredit makes the other machine change the desk by itself", async () => {
    vi.useFakeTimers();
    const store = new Store(new DemoSource(demoOptions("demo&still&peeredit")));
    await store.start();
    expect(store.toasts).toEqual([]);
    vi.advanceTimersByTime(1500);
    expect(store.toasts[0]?.text).toMatch(/^Changed from /);
  });

  it("undoing a version that does not exist changes nothing and says so", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    expect(await src.undoDesk(99)).toEqual({ ok: false, reason: expect.any(String) });
  });
});

describe("transfer notice with progress (scope 3)", () => {
  const t = (done: number, finished = false) => ({ kind: "transfer" as const, upload: true, path: "/x/wedding.zip", done, total: 100, finished });

  it("each sample updates the same notice and on finishing it ends as ok and goes away by itself", async () => {
    vi.useFakeTimers();
    const { bridge, emit } = fake();
    const store = new Store(new LiveSource(bridge));
    await store.start();
    emit(t(10));
    const id = store.toasts[0].id;
    emit(t(60));
    expect(store.toasts).toHaveLength(1);
    expect(store.toasts[0]).toMatchObject({ id, auto: false, progress: { done: 60, total: 100 } });
    vi.advanceTimersByTime(TOAST_MS * 2);
    expect(store.toasts).toHaveLength(1);
    emit(t(100, true));
    expect(store.toasts[0]).toMatchObject({ id, text: "Copied “wedding.zip”", tone: "ok", auto: true });
    vi.advanceTimersByTime(TOAST_MS + 10);
    expect(store.toasts).toEqual([]);
  });

  it("in the demo it can also be simulated", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    src.simulateTransfer({ upload: false, path: "/x/a.txt", done: 5, total: 10, finished: false });
    expect(store.toasts[0].progress).toEqual({ done: 5, total: 10 });
  });
});
