// Acceptance fixes (part B): T-153 (reactive `can()`), T-154 (tunnels on
// opening), T-157 (lock state) and T-158 (modules with real state). Simulated bridge and demo.
import { act as flush, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { buildCommands } from "../lib/commands";
import { STORE_KEY } from "../lib/context";
import { summaryRows } from "../lib/wizard";
import type { Action, ActionResult, Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store, TRAY_TOPICS } from "../lib/store/store.svelte";
import type { Source } from "../lib/store/types";
import type { ProjectWire } from "../lib/tunnels";
import Shell from "../shell/Shell.svelte";
import Tray from "../Tray.svelte";

afterEach(() => {
  vi.useRealTimers();
});

type Connected = Extract<Snapshot, { kind: "connected" }>;

function snapshot(locked = false, modules: unknown[] = []): Connected {
  return {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: { os: "macos", name: "MacBook Pro", color: "#7aa7e6" }, b: { os: "windows", name: "PC-Desktop", color: "#a99be8" } }, screens: [], arrangement: null, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Desktop", link: "usb4" },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked, missing: [], problem: null, defined: true },
    caps: {
      here: {
        machine: "a",
        name: "MacBook Pro",
        os: "macos",
        color: "#7aa7e6",
        hub: null,
        input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: true },
        services: [],
        modules,
      },
      peer: null,
    },
    monitors: [],
  } as unknown as Connected;
}

const wire: ProjectWire[] = [{ name: "tandem", root: "/src/tandem", tunnels: [{ id: "f00dfacecafe0001", title: "Review the PR", origin: "a", target: "b", open: true, created: Date.now() - 60_000, running: [] }] }];

/** A simulated bridge: the first snapshot is given by `first`; the following ones are pushed by hand. */
function bridge(first: Snapshot, over: Record<string, unknown> = {}, actions: (a: Action) => ActionResult = () => ({ kind: "done" })) {
  let push: ((s: Snapshot) => void) | undefined;
  const act = vi.fn(async (a: Action) => actions(a));
  const b = {
    snapshot: async () => first,
    onSnapshot: async (cb: (s: Snapshot) => void) => {
      push = cb;
    },
    act,
    shortcuts: async () => ({ bindings: { toggle_window: null }, failed: {}, warning: null, file: null }),
    ...over,
  } as unknown as Bridge;
  return { b, act, push: (s: Snapshot) => push?.(s) };
}

async function open(source: Source, hash: string) {
  const store = new Store(source, { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

/** The status chip (notices with the same text do not count). */
const chip = () => [...document.querySelectorAll(".chip")].find((c) => c.textContent?.includes("Cursor locked here")) ?? null;

const tunnelsReply = (a: Action): ActionResult => (a.kind === "tunnels" ? { kind: "tunnels", tunnels: wire } : { kind: "done" });

describe("T-153: buttons with can() are enabled when the agent connects", () => {
  it("\"Measure bandwidth\" is enabled when the first snapshot arrives", async () => {
    const { b, push } = bridge({ kind: "connecting" });
    await open(new LiveSource(b), "#/network");
    const measure = () => screen.getByRole("button", { name: /Measure bandwidth/ });
    expect(measure()).toBeDisabled();
    await flush(async () => push(snapshot()));
    expect(measure()).toBeEnabled();
  });

  it("\"Apply\" in Monitors is enabled when the first snapshot arrives", async () => {
    const withScene = { ...snapshot(), desk: { ...snapshot().desk, scenes: [{ id: "t", name: "Work", monitors: [], shortcut: [] }] } } as Snapshot;
    const { b, push } = bridge({ kind: "connecting" });
    await open(new LiveSource(b), "#/monitors");
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
    await flush(async () => push(withScene));
    expect(await screen.findByRole("button", { name: "Apply" })).toBeEnabled();
  });

  it("Arrangement re-evaluates can() with each snapshot; with the pair paired \"Test crossing\" is enabled (T-150)", async () => {
    const { b, push } = bridge({ kind: "connecting" });
    const src = new LiveSource(b);
    const store = await open(src, "#/arrangement");
    expect(store.can("monitors")).toBe(false);
    await flush(async () => push(snapshot()));
    // `can()` changes with the snapshot (T-153) and, with the pair paired, the real crossing is available (T-150).
    expect(store.can("monitors")).toBe(true);
    expect(store.can("crossing")).toBe(true);
  });

  it("it is disabled again if the agent is lost", async () => {
    const { b, push } = bridge(snapshot());
    await open(new LiveSource(b), "#/network");
    expect(screen.getByRole("button", { name: /Measure bandwidth/ })).toBeEnabled();
    await flush(async () => push({ kind: "unavailable", endpoint: null, reason: "no agent" }));
    expect(screen.getByRole("button", { name: /Measure bandwidth/ })).toBeDisabled();
  });

  it("in demo mode the button is enabled from the start", async () => {
    await open(new DemoSource(demoOptions("demo&still")), "#/network");
    expect(screen.getByRole("button", { name: /Measure bandwidth/ })).toBeEnabled();
  });
});

describe("T-154: the window asks for the tunnels that already exist", () => {
  it("shows them without waiting for an event and asks for them only once", async () => {
    const { b, act } = bridge(snapshot(), {}, tunnelsReply);
    const store = await open(new LiveSource(b), "#/tunnels");
    await flush(async () => {});
    expect(act).toHaveBeenCalledWith({ kind: "tunnels" });
    expect(store.desk.tunnels.map((t) => t.title)).toEqual(["Review the PR"]);
    expect(await screen.findByText("1 open · 1 project")).toBeInTheDocument();
    expect(act.mock.calls.filter(([a]) => a.kind === "tunnels")).toHaveLength(1);
  });

  it("the tray and the palette also show them after opening", async () => {
    const running = [{ id: 1, command: "cargo test", pid: 9, started: Date.now(), duration: null, state: { kind: "running" as const } }];
    const busy: ProjectWire[] = [{ ...wire[0], tunnels: [{ ...wire[0]!.tunnels[0]!, running }] } as ProjectWire];
    const reply = (a: Action): ActionResult => (a.kind === "tunnels" ? { kind: "tunnels", tunnels: busy } : { kind: "done" });
    const { b } = bridge(snapshot(), {}, reply);
    const store = new Store(new LiveSource(b), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("1 tunnel")).toBeInTheDocument());
    const titles = buildCommands(store.desk, store, false).map((c) => c.title);
    expect(titles).toContain("Go to tunnel “Review the PR”");
  });

  it("waits for an agent to exist and then asks for them", async () => {
    const { b, act, push } = bridge({ kind: "connecting" }, {}, tunnelsReply);
    const store = await open(new LiveSource(b), "#/tunnels");
    expect(act).not.toHaveBeenCalledWith({ kind: "tunnels" });
    await flush(async () => push(snapshot()));
    await flush(async () => {});
    expect(store.desk.tunnels).toHaveLength(1);
  });

  it("a late request does not overwrite what arrived by event", async () => {
    let emit: ((g: ProjectWire[]) => void) | undefined;
    let release: (() => void) | undefined;
    const { b } = bridge(snapshot(), {
      onTunnels: async (cb: (g: ProjectWire[]) => void) => {
        emit = cb;
      },
      act: (a: Action) =>
        a.kind === "tunnels"
          ? new Promise<ActionResult>((res) => {
              release = () => res({ kind: "tunnels", tunnels: [] });
            })
          : Promise.resolve({ kind: "done" }),
    });
    const store = await open(new LiveSource(b), "#/tunnels");
    await flush(async () => emit?.(wire));
    await flush(async () => release?.());
    expect(store.desk.tunnels).toHaveLength(1);
  });

  it("if the agent does not answer with tunnels, the window stays empty without breaking", async () => {
    const { b } = bridge(snapshot());
    const store = await open(new LiveSource(b), "#/tunnels");
    await flush(async () => {});
    expect(store.desk.tunnels).toEqual([]);
    expect(screen.getAllByText("No open tunnels").length).toBeGreaterThan(0);
  });
});

describe("T-157: \"Lock here\" is visible and the palette offers only what applies", () => {
  it("the Desk shows the chip while the cursor is locked and removes it on releasing", async () => {
    const { b, push } = bridge(snapshot(false));
    await open(new LiveSource(b), "#/desk");
    expect(chip()).toBeNull();
    await flush(async () => push(snapshot(true)));
    expect(chip()).not.toBeNull();
    await flush(async () => push(snapshot(false)));
    expect(chip()).toBeNull();
  });

  it("the tray shows the chip while locked and removes it on releasing", async () => {
    const { b, push } = bridge(snapshot(false));
    const store = new Store(new LiveSource(b), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(store.desk.ready).toBe(true));
    expect(chip()).toBeNull();
    await flush(async () => push(snapshot(true)));
    expect(chip()).not.toBeNull();
    await flush(async () => push(snapshot(false)));
    expect(chip()).toBeNull();
  });

  it("the palette changes order depending on the state", async () => {
    const { b, push } = bridge(snapshot(false));
    const store = await open(new LiveSource(b), "#/desk");
    const lock = () => buildCommands(store.desk, store, false).filter((c) => c.id.startsWith("lock:")).map((c) => c.title);
    expect(lock()).toEqual(["Lock here"]);
    await flush(async () => push(snapshot(true)));
    expect(lock()).toEqual(["Remove the lock"]);
  });

  it("announces on locking and releasing, and sends the order to the agent", async () => {
    const { b, act } = bridge(snapshot());
    const store = await open(new LiveSource(b), "#/desk");
    expect(await store.lockHere(true)).toEqual({ ok: true, text: "Cursor locked here" });
    expect(act).toHaveBeenLastCalledWith({ kind: "lock_here", on: true });
    expect(await store.lockHere(false)).toEqual({ ok: true, text: "Lock removed" });
  });

  it("an agent rejection is explained and does not change the state", async () => {
    const { b } = bridge(snapshot(), {}, () => ({ kind: "rejected", reason: "this machine does not have the keyboard" }));
    const store = await open(new LiveSource(b), "#/desk");
    expect(await store.lockHere(true)).toEqual({ ok: false, reason: "this machine does not have the keyboard" });
    expect(store.desk.locked).toBe(false);
  });

  it("in demo mode the chip appears on locking and is removed on releasing", async () => {
    const store = await open(new DemoSource(demoOptions("demo&still")), "#/desk");
    expect(chip()).toBeNull();
    await flush(async () => void (await store.lockHere(true)));
    expect(chip()).not.toBeNull();
    await flush(async () => void (await store.lockHere(false)));
    expect(chip()).toBeNull();
  });
});

describe("T-158: the modules show the state the agent reports", () => {
  const mods = [
    { id: "tunnels", name: "Tunnels", detail: "Receives tunnels from Claude", state: "active", switchable: false },
    { id: "files", name: "Files", detail: "Drag files across the edge", state: "active", switchable: false },
  ];

  it("does not invent \"Not in this version yet\": it uses the agent's state", async () => {
    const { b } = bridge(snapshot(false, mods));
    const store = await open(new LiveSource(b), "#/machine/a");
    const views = store.desk.machines.a?.modules ?? [];
    expect(views.map((m) => [m.id, m.state])).toEqual([["tunnels", "active"], ["files", "active"]]);
    expect(document.body.textContent).not.toContain("Not in this version yet");
  });

  it("the setup summary says that tunnels work, or why not", async () => {
    // With a ready environment on each side, the only thing that decides is the module state.
    const environments = [{ id: "sh", kind: "shell", name: "Shell", default: true, ready: true, detail: "" }];
    const both = (state: string, reason?: string) => ({
      ...snapshot(false, [{ ...mods[0], state, reason }]),
      caps: { ...snapshot().caps, here: { ...snapshot().caps.here, environments, modules: [{ ...mods[0], state, reason }] }, peer: { ...snapshot().caps.here, machine: "b", name: "PC-Desktop", os: "windows", environments, modules: [{ ...mods[0], state, reason }] } },
    }) as unknown as Snapshot;
    const rows = async (snap: Snapshot) => {
      const store = await open(new LiveSource(bridge(snap).b), "#/desk");
      return summaryRows(store.desk, { crossed: true }).find((r) => r.id === "tunnels");
    };
    expect(await rows(both("active"))).toMatchObject({ ok: true, note: "MCP ready for Claude Code on both" });
    expect(await rows(both("unavailable", "The system would not open anything"))).toMatchObject({ ok: false });
  });

  it("a true reason is shown", async () => {
    const reason = "The system would not open the file drag detector";
    const down = [{ ...mods[1], state: "unavailable", reason }];
    const { b } = bridge(snapshot(false, down));
    const store = await open(new LiveSource(b), "#/machine/a");
    expect(store.desk.machines.a?.modules[0]).toMatchObject({ state: "unavailable", detail: reason });
  });
});

