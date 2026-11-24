// T-250, T-251: a command reply and its notices travel on different channels (the Tauri
// command reply and an event), so either can arrive first. The window must order them by
// identity (the transaction id, the view's sequence), not by arrival.
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Action, ActionResult, Notice, Snapshot, TxnView } from "../lib/model";
import { LiveSource } from "../lib/store/live";
import { EXPECT_TXN_MS, Store } from "../lib/store/store.svelte";
import type { UpdateView } from "../lib/updates";
import Shell from "../shell/Shell.svelte";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const snap = {
  kind: "connected",
  agent: "a",
  desk: {
    machines: { a: { os: "macos", name: "Studio", color: "#7aa7e6" }, b: { os: "windows", name: "Workshop", color: "#6cc59a" } },
    screens: [],
    arrangement: null,
    scenes: [{ id: "game", name: "Game", monitors: [] }],
  },
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

/** What the test does not give: listeners that never fire and commands that fail as without an agent. */
function withRest(given: Record<string, unknown>): Bridge {
  return new Proxy(given, {
    get: (t, k: string) => t[k] ?? (k.startsWith("on") ? async () => {} : async () => Promise.reject(new Error(`${k}: not in this test`))),
  }) as unknown as Bridge;
}

const STEPS = [{ step: "geometry" }] as Extract<TxnView, { type: "started" }>["steps"];

/** A bridge whose `apply_scene` answers `accepted` and lets the test decide when the notices arrive. */
function txnBridge(onApply: (txn: number) => void = () => {}) {
  const notices: ((n: Notice) => void)[] = [];
  const act = vi.fn(async (a: Action): Promise<ActionResult> => {
    if (a.kind === "apply_scene") {
      onApply(7);
      return { kind: "accepted", txn: 7 };
    }
    return { kind: "done" };
  });
  const bridge = withRest({
    snapshot: async () => snap,
    act,
    onNotice: async (cb: (n: Notice) => void) => {
      notices.push(cb);
    },
  });
  const txn = (event: TxnView) => notices.forEach((l) => l({ kind: "txn", event }));
  return { bridge, act, txn };
}

async function openLive(bridge: Bridge, hash: string) {
  const store = new Store(new LiveSource(bridge), { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

/** What the editor's "Save and apply" and the card's "Apply" do. */
async function apply(store: Store) {
  if ((await store.applyScene("game")).ok) store.showProgress();
  await tick();
}

describe("T-250: Changes in progress waits for the transaction the window asked for", () => {
  it("the reply arrives before `started`: the window stays on Progress and then shows the steps", async () => {
    const { bridge, txn } = txnBridge();
    const store = await openLive(bridge, "#/monitors/scene");
    await apply(store);
    await tick();
    expect(store.route.screen).toBe("progress");
    expect(screen.getByRole("heading", { name: "Changes in progress" })).toBeInTheDocument();
    expect(screen.getByText("Starting the change…")).toBeInTheDocument();

    txn({ type: "started", txn: 7, steps: STEPS });
    await tick();
    expect(store.route.screen).toBe("progress");
    expect(await screen.findByRole("heading", { name: /^Applying the scene/ })).toBeInTheDocument();
    expect(screen.queryByText("Starting the change…")).toBeNull();

    txn({ type: "committed", txn: 7, state: { on: { a: true, b: true } } as never });
    await tick();
    expect(store.route.screen).toBe("progress");
    expect(screen.getByText(/^Scene “.*” active\.$/)).toBeInTheDocument();
  });

  it("`started` arrives before the reply (the old passing order): the steps show at once", async () => {
    let early: ((txn: number) => void) | null = null;
    const { bridge, txn } = txnBridge((id) => early?.(id));
    early = (id) => txn({ type: "started", txn: id, steps: STEPS });
    const store = await openLive(bridge, "#/monitors/scene");
    await apply(store);
    expect(store.route.screen).toBe("progress");
    expect(await screen.findByRole("heading", { name: /^Applying the scene/ })).toBeInTheDocument();
    expect(screen.queryByText("Starting the change…")).toBeNull();
  });

  it("a window that already ran a transaction does not show the old one while it waits for the new one", async () => {
    const { bridge, txn } = txnBridge();
    const store = await openLive(bridge, "#/monitors");
    txn({ type: "started", txn: 3, steps: STEPS });
    txn({ type: "committed", txn: 3, state: { on: { a: true, b: true } } as never });
    await apply(store);
    expect(store.route.screen).toBe("progress");
    expect(screen.getByText("Starting the change…")).toBeInTheDocument();
    txn({ type: "started", txn: 7, steps: STEPS });
    expect(await screen.findByRole("heading", { name: /^Applying the scene/ })).toBeInTheDocument();
  });

  it("if `started` never arrives, it says so and goes back to Monitors", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const { bridge } = txnBridge();
    const store = await openLive(bridge, "#/monitors/scene");
    await apply(store);
    expect(store.route.screen).toBe("progress");
    vi.advanceTimersByTime(EXPECT_TXN_MS + 10);
    await tick();
    await waitFor(() => expect(store.route.screen).toBe("monitors"));
    expect(store.toasts.map((t) => [t.text, t.tone])).toContainEqual(["The change was accepted, but its progress did not arrive", "bad"]);
  });

  it("the bare address with nothing expected still goes back to Monitors", async () => {
    const { bridge } = txnBridge();
    const store = await openLive(bridge, "#/progress");
    await waitFor(() => expect(store.route.screen).toBe("monitors"));
  });
});

const BASE: UpdateView = { seq: 1, here: "0.1.0", peer: "0.1.0", paired: true, available: "0.2.0", checking: false, step: null, error: null, auto: true, admin: false, updated_from: null };

/** The app core of `updater.rs`: the step events of the run leave before the reply, which carries the older view. */
function updatesBridge() {
  let onUpdate: ((v: UpdateView) => void) | null = null;
  const updateAll = vi.fn(async (): Promise<UpdateView> => {
    if (updateAll.mock.calls.length > 1) throw new Error("an update is already in progress");
    onUpdate?.({ ...BASE, seq: 2 });
    onUpdate?.({ ...BASE, seq: 3, step: "ask_peer" });
    onUpdate?.({ ...BASE, seq: 4, step: "wait_peer" });
    return { ...BASE, seq: 2 };
  });
  const bridge = withRest({
    snapshot: async () => snap,
    act: async () => ({ kind: "done" }),
    updates: async () => BASE,
    onUpdates: async (cb: (v: UpdateView) => void) => {
      onUpdate = cb;
    },
    checkUpdates: async () => BASE,
    updateAll,
    setAutoUpdate: async (on: boolean) => ({ ...BASE, seq: 9, auto: on }),
  });
  return { bridge, updateAll, emit: (v: UpdateView) => onUpdate?.(v) };
}

async function openUpdates(bridge: Bridge) {
  const store = await openLive(bridge, "#/settings");
  await userEvent.click(screen.getByRole("tab", { name: "Updates" }));
  await screen.findByText("Tandem 0.1.0");
  return store;
}

describe("T-251: a stale reply does not overwrite newer update steps", () => {
  it("\"Update all\" shows the current step and disables the button although the reply is older", async () => {
    const { bridge } = updatesBridge();
    await openUpdates(bridge);
    await userEvent.click(screen.getByRole("button", { name: /Update all/ }));
    expect(await screen.findByText("The other machine is updating; this one keeps the keyboard and mouse…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Update all/ })).toBeDisabled();
  });

  it("pressing again does nothing: no second command and no console error", async () => {
    const error = vi.spyOn(console, "error");
    const { bridge, updateAll } = updatesBridge();
    await openUpdates(bridge);
    const button = screen.getByRole("button", { name: /Update all/ });
    await userEvent.click(button);
    await userEvent.click(button);
    expect(updateAll).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(error).not.toHaveBeenCalled();
  });

  it("the run's end (a newer event) is shown, and the old reply of a later command does not undo it", async () => {
    const { bridge, emit } = updatesBridge();
    await openUpdates(bridge);
    await userEvent.click(screen.getByRole("button", { name: /Update all/ }));
    emit({ ...BASE, seq: 5, step: null, error: "the other machine did not come back" });
    expect(await screen.findByText("the other machine did not come back")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Update all/ })).toBeEnabled();
    // An event older than what is shown is ignored.
    emit({ ...BASE, seq: 4, step: "wait_peer" });
    await tick();
    expect(screen.getByRole("button", { name: /Update all/ })).toBeEnabled();
  });
});
