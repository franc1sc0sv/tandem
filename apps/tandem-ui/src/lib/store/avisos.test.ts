// D5: el recorrido del aviso «Cambiado desde X · Deshacer», de punta a punta, por el agente (IPC
// simulada) y por el modo demo; y el tope y el cierre de los avisos.
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
  desk: { machines: { a: { os: "macos", name: "Estudio", color: "#7aa7e6" }, b: { os: "windows", name: "Taller", color: "#6cc59a" } }, screens: [], arrangement: null, scenes: [] },
  state: { on: { a: true, b: true } },
  peer: { kind: "paired", name: "Taller", link: "usb4" },
  pairing: { kind: "idle" },
  monitors: [],
  input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
  caps: {
    here: {
      machine: "a",
      name: "Estudio",
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

describe("aviso de cambio del puesto (D5) por el agente", () => {
  it("desk_changed -> aviso -> «Deshacer» -> undo_desk con la versión anterior", async () => {
    const { bridge, act, emit } = fake();
    const store = new Store(new LiveSource(bridge));
    await store.start();
    emit({ kind: "desk_changed", by: "b", rev: 7, summary: "Arrangement", undo_to: 6 });
    expect(store.toasts).toHaveLength(1);
    expect(store.toasts[0]).toMatchObject({ text: "Changed from Taller", body: "Arrangement", tone: "info", auto: false });
    expect(store.toasts[0].action?.label).toBe("Undo");

    store.toasts[0].action?.run();
    await vi.waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "undo_desk", to_rev: 6 }));
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Change undone", tone: "ok" }));
  });

  it("el aviso con «Deshacer» no se va solo; los demás sí a los 6 s", async () => {
    vi.useFakeTimers();
    const { bridge, emit } = fake();
    const store = new Store(new LiveSource(bridge));
    await store.start();
    emit({ kind: "desk_changed", by: "b", rev: 2, summary: "Shortcuts", undo_to: 1 });
    store.toast("Done");
    vi.advanceTimersByTime(TOAST_MS + 100);
    expect(store.toasts.map((t) => t.text)).toEqual(["Changed from Taller"]);
    store.dismiss(store.toasts[0].id);
    expect(store.toasts).toEqual([]);
  });

  it("un aviso retenido (`hold`, la pila de ejemplo de F-Notices) no se va solo", async () => {
    vi.useFakeTimers();
    const { bridge } = fake();
    const store = new Store(new LiveSource(bridge));
    await store.start();
    store.notify({ text: "Fijo", tone: "ok", hold: true });
    store.notify({ text: "Normal", tone: "ok" });
    expect(store.toasts.map((t) => t.auto)).toEqual([true, false]);
    vi.advanceTimersByTime(TOAST_MS + 100);
    expect(store.toasts.map((t) => t.text)).toEqual(["Fijo"]);
  });

  it("si el agente rechaza deshacer, avisa la razón", async () => {
    const { bridge, act, emit } = fake();
    act.mockImplementation(async (a: unknown) =>
      (a as { kind: string }).kind === "undo_desk" ? ({ kind: "rejected", reason: "esa versión ya no está" } as never) : ({ kind: "done" } as never),
    );
    const store = new Store(new LiveSource(bridge));
    await store.start();
    emit({ kind: "desk_changed", by: "b", rev: 3, summary: "Arrangement", undo_to: 2 });
    store.toasts[0].action?.run();
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "esa versión ya no está", tone: "bad" }));
  });

  it("la bandeja (sin ventana principal) no repite el aviso", async () => {
    const { bridge, emit } = fake();
    const store = new Store(new LiveSource(bridge), null, TRAY_TOPICS);
    await store.start();
    emit({ kind: "desk_changed", by: "b", rev: 3, summary: "Arrangement", undo_to: 2 });
    expect(store.toasts).toEqual([]);
  });

  it("la pila guarda a lo más TOAST_MAX avisos y el más nuevo va arriba", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    for (let i = 0; i < TOAST_MAX + 3; i++) store.toast(`aviso ${i}`);
    expect(store.toasts).toHaveLength(TOAST_MAX);
    expect(store.toasts[0].text).toBe(`aviso ${TOAST_MAX + 2}`);
  });
});

describe("aviso de cambio del puesto (D5) en modo demo", () => {
  it("the other machine cambia el puesto, avisa y «Deshacer» restaura el estado anterior", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    const antes = store.desk.ready ? store.desk.prefs : null;
    expect(antes).not.toBeNull();

    src.peerEdit("Clipboard apagado");
    expect(store.desk.ready && store.desk.prefs.clipboard).toBe(!antes?.clipboard);
    expect(store.toasts[0]).toMatchObject({ body: "Clipboard apagado", auto: false });
    expect(store.toasts[0].text).toMatch(/^Changed from /);

    store.toasts[0].action?.run();
    await vi.waitFor(() => expect(store.desk.ready && store.desk.prefs).toEqual(antes));
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Change undone", tone: "ok" }));
  });

  it("?peeredit hace que the other machine cambie el puesto sola", async () => {
    vi.useFakeTimers();
    const store = new Store(new DemoSource(demoOptions("demo&still&peeredit")));
    await store.start();
    expect(store.toasts).toEqual([]);
    vi.advanceTimersByTime(1500);
    expect(store.toasts[0]?.text).toMatch(/^Changed from /);
  });

  it("deshacer una versión que no existe no cambia nada y lo dice", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    expect(await src.undoDesk(99)).toEqual({ ok: false, reason: expect.any(String) });
  });
});

describe("aviso de transferencia con progreso (alcance 3)", () => {
  const t = (done: number, finished = false) => ({ kind: "transfer" as const, upload: true, path: "/x/boda.zip", done, total: 100, finished });

  it("cada muestra actualiza el mismo aviso y al terminar queda en ok y se va solo", async () => {
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
    expect(store.toasts[0]).toMatchObject({ id, text: "Copied “boda.zip”", tone: "ok", auto: true });
    vi.advanceTimersByTime(TOAST_MS + 10);
    expect(store.toasts).toEqual([]);
  });

  it("en el demo también se puede simular", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    src.simulateTransfer({ upload: false, path: "/x/a.txt", done: 5, total: 10, finished: false });
    expect(store.toasts[0].progress).toEqual({ done: 5, total: 10 });
  });
});
