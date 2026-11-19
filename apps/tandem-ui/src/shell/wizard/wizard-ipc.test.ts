import { fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import type { Bridge } from "../../lib/bridge";
import { STORE_KEY } from "../../lib/context";
import type { Action, ActionResult, AdminView, Notice, Snapshot } from "../../lib/model";
import { LiveSource } from "../../lib/store/live";
import { Store } from "../../lib/store/store.svelte";
import Shell from "../Shell.svelte";

// The setup against the real source (`LiveSource`) and a simulated bridge: what the user does
// in each step reaches the agent as the matching IPC request.

type Connected = Extract<Snapshot, { kind: "connected" }>;

/** A Mac (this machine) with Accessibility not granted and a PC paired over USB-C. */
function snapshot(over: Partial<Connected> = {}): Connected {
  return {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: null, b: { os: "windows", name: "PC-Desktop", color: "#a99be8" } }, screens: [], arrangement: { a_side: "right", align: "start" }, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Desktop", link: "usb4" },
    pairing: { kind: "idle" },
    monitors: [],
    input: { server: "a", hub_here: null, session: true, locked: false, missing: ["accessibility"], problem: null, defined: true },
    caps: {
      here: {
        machine: "a",
        name: "MacBook Pro",
        os: "macos",
        color: "#7aa7e6",
        hub: null,
        input: { capture: false, inject: false, missing: ["accessibility"], process: { exe: "/x/tandemd", pid: 7, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: false },
        services: [],
      },
      peer: null,
    },
    ...over,
  };
}

async function mount(step: number, snap: Connected = snapshot(), onAct?: (a: Action) => Promise<void>) {
  const calls: Action[] = [];
  const act = vi.fn(async (a: Action): Promise<ActionResult> => {
    if (onAct) await onAct(a);
    else calls.push(a);
    return { kind: "done" };
  });
  const openSettings = vi.fn(async () => {});
  const restartAgent = vi.fn(async () => {});
  let adminState: AdminView = { state: "pending", canInstall: true, note: "" };
  const adminStatus = vi.fn(async () => adminState);
  const installAdmin = vi.fn(async () => {
    adminState = { state: "ok", canInstall: false, note: "" };
    return adminState;
  });
  let notice: ((n: Notice) => void) | undefined;
  let push: ((s: Snapshot) => void) | undefined;
  const bridge = {
    snapshot: async () => snap,
    onSnapshot: async (cb: (s: Snapshot) => void) => {
      push = cb;
    },
    onNotice: async (cb: (n: Notice) => void) => {
      notice = cb;
    },
    shortcuts: async () => ({ bindings: {}, failed: {}, warning: null, file: null }),
    onShortcuts: async () => {},
    onResources: async () => {},
    onNetwork: async () => {},
    onTunnels: async () => {},
    onTransfer: async () => {},
    onMessage: async () => {},
    act,
    openSettings,
    restartAgent,
    adminStatus,
    installAdmin,
  } as unknown as Bridge;
  const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
  await store.start();
  store.openWizard(step);
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  const dlg = screen.getByRole("dialog", { name: "Setup" });
  const kinds = () => calls.map((c) => c.kind);
  return { ...r, store, dlg, act, calls, kinds, openSettings, restartAgent, adminStatus, installAdmin, notice: (n: Notice) => notice?.(n), push: (s: Snapshot) => push?.(s) };
}

describe("the setup against the agent (real IPC)", () => {
  it("step 1: the name and color are saved with `set_machine`", async () => {
    const { dlg, act } = await mount(0);
    const input = within(dlg).getByRole("textbox", { name: "Name" });
    await userEvent.clear(input);
    await userEvent.type(input, "Laptop{Enter}");
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "a", name: "Laptop" }));
    await userEvent.click(within(dlg).getByRole("radio", { name: "Amber" }));
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "a", color: "#d9a75a" }));
  });

  it("step 1: \"Install\" calls the bridge and the row takes the state it returns", async () => {
    const { dlg, installAdmin } = await mount(0);
    const found = within(dlg).getByRole("region", { name: "What it found" });
    await userEvent.click(await within(found).findByRole("button", { name: "Install" }));
    expect(installAdmin).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(within(found).queryByRole("button", { name: "Install" })).toBeNull());
  });

  it("step 1: if the user cancels, it warns and the row goes back to its real state without spinning", async () => {
    const { dlg, installAdmin, adminStatus } = await mount(0);
    installAdmin.mockRejectedValueOnce(new Error("the administrator request was canceled"));
    const found = within(dlg).getByRole("region", { name: "What it found" });
    await userEvent.click(await within(found).findByRole("button", { name: "Install" }));
    expect(await screen.findByText("the administrator request was canceled")).toBeInTheDocument();
    await waitFor(() => expect(adminStatus.mock.calls.length).toBeGreaterThan(1));
    expect(within(found).getByRole("button", { name: "Install" })).toBeEnabled();
  });

  it("step 2: unpaired it searches by itself and confirms the code the user types", async () => {
    const { dlg, act, push, kinds } = await mount(1, snapshot({ peer: { kind: "unpaired" } }));
    await waitFor(() => expect(kinds()).toContain("start_pairing"));
    push(snapshot({ peer: { kind: "unpaired" }, pairing: { kind: "comparing", code: "418027", peer: "PC-Desktop", confirmed: false } }));
    await tick();
    for (const [i, d] of [..."418027"].entries()) await userEvent.type(within(dlg).getByRole("textbox", { name: `Digit ${i + 1}` }), d);
    await waitFor(() => expect(act).toHaveBeenLastCalledWith({ kind: "confirm_pairing" }));
  });

  it("step 2: \"Change the other machine\" unpairs with `unpair` only after confirming", async () => {
    const { dlg, kinds } = await mount(1);
    await userEvent.click(within(dlg).getByRole("button", { name: "Change the other machine" }));
    expect(kinds()).not.toContain("unpair");
    await userEvent.click(within(dlg).getByRole("button", { name: "Unpair" }));
    await waitFor(() => expect(kinds()).toContain("unpair"));
  });

  it("step 3: opens the permission panel and restarts the agent through the bridge", async () => {
    const { dlg, openSettings, restartAgent } = await mount(2);
    expect(within(dlg).getByRole("heading", { name: "macOS permissions" })).toBeInTheDocument();
    const next = within(dlg).getByRole("button", { name: "Continue" });
    expect(next).toHaveClass("wait");
    await userEvent.click(within(dlg).getAllByRole("button", { name: "Open settings" })[0]!);
    expect(openSettings).toHaveBeenCalledWith("accessibility");
    await userEvent.click(await within(dlg).findByRole("button", { name: "Restart agent" }));
    expect(restartAgent).toHaveBeenCalledTimes(1);
  });

  it("step 3: when the agent reports the permission granted, Continue is enabled", async () => {
    const { dlg, push } = await mount(2);
    expect(within(dlg).getByRole("button", { name: "Continue" })).toHaveClass("wait");
    const ok = snapshot();
    ok.input = { ...ok.input, missing: [] };
    ok.caps = { ...ok.caps, here: { ...ok.caps.here, input: { ...ok.caps.here.input, capture: true, inject: true, missing: [] } } };
    push(ok);
    await tick();
    await waitFor(() => expect(within(dlg).getByRole("button", { name: "Continue" })).not.toHaveClass("wait"));
  });

  it("step 4: turns on the key probe, marks the machine that heard and turns it off on leaving", async () => {
    const { dlg, notice, act, unmount } = await mount(3);
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "key_probe", on: true }));
    const pc = within(dlg).getByRole("group", { name: "PC-Desktop" });
    expect(within(pc).queryByText("Activity detected")).toBeNull();
    notice({ kind: "key_probe", event: { armed: true, heard: "b" } });
    expect(await within(pc).findByText("Activity detected")).toBeInTheDocument();
    unmount();
    await waitFor(() => expect(act).toHaveBeenLastCalledWith({ kind: "key_probe", on: false }));
  });

  it("step 4: this window's activity is marked and reported to the agent with `heard_key`", async () => {
    const { dlg, act, calls } = await mount(3);
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    await fireEvent.keyDown(window, { key: "a" });
    expect(await within(mac).findByText("Activity detected")).toBeInTheDocument();
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "heard_key" }));
    await fireEvent.keyDown(window, { key: "b" });
    expect(calls.filter((c) => c.kind === "heard_key")).toHaveLength(1);
  });

  it("step 4: lists the keyboard and mouse only if the agent announces the hub here", async () => {
    const without = await mount(3);
    const macWithout = within(without.dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(macWithout).getByText("No keyboard or mouse right now")).toBeInTheDocument();
    expect(within(macWithout).queryByText("Keyboard and mouse")).toBeNull();
    without.unmount();
    const base = snapshot();
    const withHub = await mount(3, snapshot({ caps: { ...base.caps, here: { ...base.caps.here, hub: true } } }));
    const macWith = within(withHub.dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(macWith).getByText("Keyboard and mouse")).toBeInTheDocument();
    expect(within(macWith).queryByText("No keyboard or mouse right now")).toBeNull();
  });

  it("\"Close\" exists with a name, closes with a click and its CSS gives it a box of 28 px or more", async () => {
    const { dlg, store } = await mount(1);
    const close = within(dlg).getByRole("button", { name: "Close" });
    expect(close.textContent?.trim()).toBe("");
    const css = readFileSync(join(process.cwd(), "src", "shell", "Wizard.svelte"), "utf8");
    const rule = /\.wz-close \{([^}]*)\}/.exec(css)?.[1] ?? "";
    expect(Number(/width:\s*(\d+)px/.exec(rule)?.[1])).toBeGreaterThanOrEqual(28);
    expect(Number(/height:\s*(\d+)px/.exec(rule)?.[1])).toBeGreaterThanOrEqual(28);
    expect(rule).not.toContain("absolute");
    expect(css).toContain(".wz-close:focus-visible");
    await userEvent.click(close);
    expect(store.wizardOpen).toBe(false);
  });

  it("step 4: the key that arrives before the probe is armed is reported when it is armed", async () => {
    const calls: Action[] = [];
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => (release = r));
    const { dlg, act } = await mount(3, snapshot(), async (a) => {
      if (a.kind === "key_probe") await gate;
      calls.push(a);
    });
    await fireEvent.keyDown(window, { key: "a" });
    expect(calls.some((c) => c.kind === "heard_key")).toBe(false);
    release();
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "heard_key" }));
    expect(within(dlg).getAllByText("Activity detected").length).toBeGreaterThan(0);
  });

  it("step 4: the row of the other machine also comes from its hub", async () => {
    const base = snapshot();
    const peer = { ...base.caps.here, machine: "b" as const, name: "PC-Desktop", os: "windows" as const, hub: true };
    const { dlg } = await mount(3, snapshot({ caps: { ...base.caps, peer } }));
    expect(within(within(dlg).getByRole("group", { name: "PC-Desktop" })).getByText("Keyboard and mouse")).toBeInTheDocument();
    expect(within(within(dlg).getByRole("group", { name: "MacBook Pro" })).getByText("No keyboard or mouse right now")).toBeInTheDocument();
  });

  it("step 5: moving the other machine saves the arrangement with `set_arrangement`", async () => {
    const { dlg, act } = await mount(4);
    const other = within(dlg).getByRole("button", { name: /Move PC-Desktop/ });
    other.focus();
    await userEvent.keyboard("{ArrowDown}");
    await waitFor(() => expect(act).toHaveBeenCalledWith(expect.objectContaining({ kind: "set_arrangement", a_side: "up" })));
  });

  it("step 5: \"Test crossing\" asks the agent for the cursor and a Crossed through the marked edge passes it", async () => {
    const { dlg, store, act, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true }));
    notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
    notice({ kind: "cursor", sample: { machine: "b", screen: "m34", x: 3, y: 720 } });
    expect(await within(dlg).findByText(/Crossed to PC-Desktop through the left edge, as the arrangement says/)).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(true);
  });

  it("step 5: a Crossed through another edge gives \"Crossed at another edge\" and does not enable continuing", async () => {
    const { dlg, store, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    await waitFor(() => expect(store.wizardCrossed).toBe(false));
    notice({ kind: "crossed", from: "a", to: "b", edge: "right" });
    expect(await within(dlg).findByText(/Crossed at the right, not the left/, {}, { timeout: 2000 })).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(false);
  });

  it("step 5: a Crossed without a cursor sample passes without a position or an invented latency", async () => {
    const { dlg, store, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    await waitFor(() => expect(store.wizardCrossed).toBe(false));
    notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
    const text = await within(dlg).findByText(/Crossed to PC-Desktop through the left edge, as the arrangement says/, {}, { timeout: 2000 });
    expect(text.textContent).not.toMatch(/Round trip measured/);
    expect(store.wizardCrossed).toBe(true);
  });

  it("step 5: without any crossing in 20 s the test says \"Did not cross\" and does not enable continuing", async () => {
    const { dlg, store, act } = await mount(4);
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    try {
      await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }), { advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
      await vi.advanceTimersByTimeAsync(20_500);
      expect(await within(dlg).findByText(/Did not cross in 20 s/)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
    expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true });
    expect(store.wizardCrossed).toBe(false);
  });
});
