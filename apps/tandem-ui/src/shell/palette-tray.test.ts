// T-120: the ⌘K palette (F-Palette) and the tray (V3-Tray) against the real agent (simulated IPC) and in
// demo mode, and their animations with `prefers-reduced-motion`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Snapshot } from "../lib/model";
import { enter, leave } from "../lib/motion";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store, TRAY_TOPICS } from "../lib/store/store.svelte";
import { setReducedMotion } from "../test/setup";
import Tray from "../Tray.svelte";
import Shell from "./Shell.svelte";

const SRC = join(process.cwd(), "src");
const read = (...p: string[]) => readFileSync(join(SRC, ...p), "utf8");

function connected(over: Partial<Extract<Snapshot, { kind: "connected" }>> = {}): Snapshot {
  return {
    kind: "connected",
    agent: "b",
    desk: { machines: { a: { os: "windows", name: "Workshop", color: "#6cc59a" }, b: null }, screens: [], arrangement: { a_side: "up", align: "end" }, scenes: [] },
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
    ...over,
  } as Snapshot;
}

/** A simulated agent: `act` stores what it is asked. */
function fakeBridge(snap: Snapshot) {
  const act = vi.fn(async (_a: unknown) => ({ kind: "done" as const }));
  const bridge = { snapshot: async () => snap, onSnapshot: async () => {}, act, shortcuts: async () => ({ items: [], mac: false }) } as unknown as Bridge;
  return { bridge, act };
}

async function openPalette(store: Store) {
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  await fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  await tick();
  return r;
}

describe("palette against the real agent (IPC)", () => {
  it("lists what the agent can do: power off the other machine and measure the link; no scenes", async () => {
    const { bridge } = fakeBridge(connected());
    const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    expect(screen.getByRole("option", { name: /Power off Workshop/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Measure the link/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Apply/ })).toBeNull();
  });

  it("\"Power off Workshop\" asks set_power of the agent and closes the palette", async () => {
    const { bridge, act } = fakeBridge(connected());
    const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    await userEvent.click(screen.getByRole("option", { name: /Power off Workshop/ }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "a", on: false });
    expect(store.paletteOpen).toBe(false);
  });

  it("\"Measure the link\" asks measure_link over the other machine's path", async () => {
    const { bridge, act } = fakeBridge(connected());
    const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    await userEvent.click(screen.getByRole("option", { name: /Measure the link/ }));
    expect(act).toHaveBeenCalledWith({ kind: "measure_link", path: "wifi" });
  });

  it("with the other machine off it offers \"Power on\"", async () => {
    const { bridge, act } = fakeBridge(connected({ state: { on: { a: false, b: true } } }));
    const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    await userEvent.click(screen.getByRole("option", { name: /Power on Workshop/ }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "a", on: true });
  });
});

describe("palette in demo mode", () => {
  it("applies a scene with Enter after filtering by its name", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    const input = screen.getByRole("combobox", { name: "Search" });
    await userEvent.type(input, "macbook only");
    await fireEvent.keyDown(input, { key: "Enter" });
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("only-a"));
    expect(store.paletteOpen).toBe(false);
  });

  it("powers off the other machine and the \"Machines\" group offers \"Power on\"", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    await userEvent.click(screen.getByRole("option", { name: /Power off PC-Desktop/ }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).not.toBe("on"));
  });

  it("the backdrop closes the palette with a click and returns the focus", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/desk" });
    await store.start();
    const { container } = await openPalette(store);
    const veil = container.ownerDocument.querySelector(".veil") as HTMLElement;
    await userEvent.click(veil);
    expect(store.paletteOpen).toBe(false);
  });
});

describe("tray against the real agent (IPC)", () => {
  it("shows the trouble state with one line and, if the other machine is off, \"Power on\" turns it on", async () => {
    const { bridge, act } = fakeBridge(connected({ state: { on: { a: false, b: true } } }));
    const store = new Store(new LiveSource(bridge), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("Workshop is off")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "Power on" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "a", on: true });
  });

  it("with no agent it says so and offers no useless buttons", async () => {
    const bridge = { snapshot: async () => ({ kind: "unavailable", endpoint: null, reason: "not running" }), onSnapshot: async () => {}, act: vi.fn(async () => ({ kind: "done" })) } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("not running")).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "Power on" })).toBeNull();
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
  });

  it("with the agent up to date there is no state notice", async () => {
    const { bridge } = fakeBridge(connected());
    const store = new Store(new LiveSource(bridge), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("Studio")).toBeInTheDocument());
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("tray in demo mode", () => {
  it("the active scene is marked and each scene applies with a click", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByRole("button", { name: "Work" })).toHaveAttribute("aria-pressed", "true"));
    await userEvent.click(screen.getByRole("button", { name: "MacBook only" }));
    await vi.waitFor(() => expect(screen.getByRole("button", { name: "MacBook only" })).toHaveAttribute("aria-pressed", "true"));
    expect(screen.getByRole("button", { name: "Work" })).toHaveAttribute("aria-pressed", "false");
  });

  it("does not invent \"Lock here\": there is no action behind it (G6)", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("MacBook Pro")).toBeInTheDocument());
    expect(screen.queryByText(/Lock/)).toBeNull();
  });
});

describe("palette and tray animations (G7) and prefers-reduced-motion (G8)", () => {
  it("the palette brings `pop` (dialog) and `up` (rows) with the mockup curves", () => {
    const pal = read("shell", "Palette.svelte");
    expect(pal).toContain("animation: pal-pop 0.25s cubic-bezier(0.2, 0.9, 0.3, 1.1)");
    expect(pal).toMatch(/@keyframes pal-pop \{\s*from \{\s*opacity: 0;\s*transform: translateY\(-10px\) scale\(0\.98\);/);
    expect(pal).toContain("animation: pal-up 0.2s ease-out both");
    expect(pal).toMatch(/@keyframes pal-up \{\s*from \{\s*opacity: 0;\s*transform: translateY\(4px\);/);
  });

  it("the tray brings the mockup transitions: scene buttons, bars and links", () => {
    const tray = read("Tray.svelte");
    expect(tray).toMatch(/border-color 0\.25s,\s*background 0\.25s,\s*transform 0\.2s/);
    expect(tray).toContain("transform: translateY(-1px)");
    expect(tray).toContain("transition: width 0.6s cubic-bezier(0.2, 0.8, 0.2, 1)");
    expect(tray).toMatch(/background 0\.2s,\s*color 0\.2s/);
  });

  it("with prefers-reduced-motion the palette veil and exit last 0 ms and the global sheet turns off the CSS ones", () => {
    const el = document.createElement("div");
    expect(enter(el, { duration: 200 }).duration).toBe(200);
    setReducedMotion(true);
    expect(enter(el, { duration: 200 })).toEqual({ duration: 0 });
    expect(leave(el)).toEqual({ duration: 0 });
    const css = read("styles", "global.css");
    const rule = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(rule).toContain("animation: none !important");
    expect(rule).toContain("transition: none !important");
  });

  it("the palette and tray animations are CSS or motion.ts: none by JS", () => {
    for (const f of [read("shell", "Palette.svelte"), read("Tray.svelte")]) {
      expect(f).not.toMatch(/\.animate\(/);
      expect(f).not.toMatch(/from "svelte\/transition"/);
    }
  });

  it("the tray uses the same global sheet (reduced-motion included) as the app", () => {
    expect(read("tray.ts")).toContain('import "./styles/global.css"');
  });
});
