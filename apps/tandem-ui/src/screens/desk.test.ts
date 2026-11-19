// Desk screen (T-113): what it shows and what can be done, in demo mode and against a
// source that talks to the agent (simulated IPC).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Action, Notice, Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import { setReducedMotion } from "../test/setup";
import Desk from "./Desk.svelte";

afterEach(() => {
  vi.useRealTimers();
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
});

async function mount(query = "demo&still") {
  const store = new Store(new DemoSource(demoOptions(query)));
  await store.start();
  const r = render(Desk, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return { ...r, store };
}

const dot = (c: HTMLElement) => c.querySelector<HTMLElement>(".curdot");

describe("V3-Desk in demo mode", () => {
  it("shows the two machines, the link, the desktops and the scenes", async () => {
    await mount();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Desk");
    const machines = within(screen.getByRole("region", { name: "Machines" }));
    expect(machines.getByRole("link", { name: /MacBook Pro/ })).toHaveAttribute("href", "#/machine/a");
    expect(machines.getByRole("link", { name: /PC-Desktop/ })).toHaveAttribute("href", "#/machine/b");
    expect(screen.getByRole("link", { name: /Link between MacBook Pro and PC-Desktop: USB-C/ })).toHaveAttribute("href", "#/network");
    expect(screen.getByText("9.4 Gbps · 0.3 ms")).toBeInTheDocument();
    expect(screen.getByText("p99 1.9 ms")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /To scale: MacBook Pro and 34" · PC-Desktop/ })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { pressed: true })).toHaveLength(1);
    expect(screen.getByRole("button", { name: /Work/, pressed: true })).toBeInTheDocument();
  });

  it("applying a scene marks it as active", async () => {
    await mount();
    await userEvent.click(screen.getByRole("button", { name: /MacBook only/ }));
    expect(screen.getByRole("button", { name: /MacBook only/, pressed: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Work/, pressed: false })).toBeInTheDocument();
  });

  it("the icon buttons lead to arrangement and monitors", async () => {
    await mount();
    expect(screen.getByRole("link", { name: "Edit arrangement" })).toHaveAttribute("href", "#/arrangement");
    expect(screen.getByRole("link", { name: "New scene" })).toHaveAttribute("href", "#/monitors");
    expect(screen.getByRole("link", { name: "Edit scenes" })).toHaveAttribute("href", "#/monitors");
  });

  it("power the other machine off and on from its power card", async () => {
    await mount();
    const card = within(screen.getByRole("group", { name: "Power of PC-Desktop" }));
    await userEvent.click(card.getByRole("button", { name: "Power off" }));
    expect(await card.findByRole("button", { name: "Power on" })).toBeEnabled();
    await userEvent.click(card.getByRole("button", { name: "Power on" }));
    expect(await card.findByRole("button", { name: "Power off" })).toBeInTheDocument();
  });

  it("does not offer power if the other machine is not paired or is already lost", async () => {
    await mount("demo&still&trouble=link-down");
    expect(screen.queryByRole("group", { name: "Power of PC-Desktop" })).toBeNull();
  });

  it("the cursor dot appears with the first sample and the edge does not flash by itself", async () => {
    const { container } = await mount();
    await vi.waitFor(() => expect(dot(container)).not.toBeNull());
    expect(dot(container)?.style.left).toMatch(/^16(\.\d+)?%$/);
    expect(container.querySelector(".edge.on")).toBeNull();
  });

  it("with other machines (D2) the names come from each one", async () => {
    await mount("demo=win-win&still");
    expect(screen.getByRole("img", { name: /To scale:/ }).getAttribute("aria-label")).not.toContain("MacBook");
  });
});

describe("against the agent (IPC)", () => {
  const machine = (m: "a" | "b", name: string, os: string, color: string) => ({
    machine: m,
    name,
    os,
    color,
    hub: null,
    input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
    power: { wake_peer: true, wakeable: true, power_off: true },
    services: [],
    modules: [],
    environments: [],
  });
  const snap = {
    kind: "connected",
    agent: "a",
    desk: {
      machines: { a: null, b: null },
      screens: [
        { machine: "a", id: "mac", x: 0, y: 0, width: 1512, height: 982, scale: 2 },
        { machine: "b", id: "g34", x: 0, y: 0, width: 3440, height: 1440, scale: 1 },
      ],
      arrangement: { a_side: "left", align: "end" },
      scenes: [],
    },
    monitors: [],
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "Workshop", link: "usb4" },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
    caps: { here: machine("a", "Studio", "macos", "#7aa7e6"), peer: machine("b", "Workshop", "windows", "#a99be8") },
  } as unknown as Snapshot;

  async function mountLive(reply: "done" | "rejected" = "done") {
    const act = vi.fn(async (a: Action) => (reply === "rejected" && a.kind === "watch_cursor" ? { kind: "rejected" as const, reason: "no" } : { kind: "done" as const }));
    let notice: ((n: Notice) => void) | undefined;
    const bridge = {
      snapshot: async () => snap,
      onSnapshot: async () => {},
      onResources: async () => {},
      onNotice: async (cb: (n: Notice) => void) => {
        notice = cb;
      },
      act,
    } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    const r = render(Desk, { context: new Map([[STORE_KEY, store]]) });
    await tick();
    return { ...r, act, send: (n: Notice) => notice?.(n) };
  }

  const watches = (act: ReturnType<typeof vi.fn>) => act.mock.calls.map((c) => c[0]).filter((a: Action) => a.kind === "watch_cursor");

  it("asks for the live cursor and draws each sample on its screen", async () => {
    const { act, send, container } = await mountLive();
    await vi.waitFor(() => expect(watches(act)).toEqual([{ kind: "watch_cursor", on: true }]));
    expect(dot(container)).toBeNull();
    // The center of the 34" screen (3440 × 1440).
    send({ kind: "cursor", sample: { machine: "b", screen: "g34", x: 1720, y: 720 } });
    await tick();
    const left = parseFloat(dot(container)?.style.left ?? "NaN");
    const top = parseFloat(dot(container)?.style.top ?? "NaN");
    expect(left).toBeGreaterThan(55);
    expect(left).toBeLessThan(70);
    expect(top).toBeGreaterThan(30);
    expect(top).toBeLessThan(50);
  });

  it("each crossing flashes the edge", async () => {
    const { act, send, container } = await mountLive();
    await vi.waitFor(() => expect(watches(act)).toHaveLength(1));
    expect(container.querySelector(".edge.on")).toBeNull();
    send({ kind: "crossed", from: "a", to: "b", edge: "right" });
    await tick();
    expect(container.querySelector(".edge.on")).not.toBeNull();
  });

  it("only receives the cursor with the window visible", async () => {
    const { act } = await mountLive();
    await vi.waitFor(() => expect(watches(act)).toHaveLength(1));
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.waitFor(() => expect(watches(act).at(-1)).toEqual({ kind: "watch_cursor", on: false }));
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.waitFor(() => expect(watches(act).at(-1)).toEqual({ kind: "watch_cursor", on: true }));
  });

  it("on leaving the screen it stops receiving it", async () => {
    const { act, unmount } = await mountLive();
    await vi.waitFor(() => expect(watches(act)).toHaveLength(1));
    unmount();
    expect(watches(act).at(-1)).toEqual({ kind: "watch_cursor", on: false });
  });

  it("if the agent does not offer it, the desktops are seen without a dot", async () => {
    const { container } = await mountLive("rejected");
    await tick();
    expect(container.querySelector(".scale")).not.toBeNull();
    expect(dot(container)).toBeNull();
  });

  it("powering off the other machine goes to the agent", async () => {
    const { act } = await mountLive();
    await userEvent.click(screen.getByRole("button", { name: "Power off" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "b", on: false });
  });
});

describe("animations (G7, G8)", () => {
  const css = readFileSync(join(process.cwd(), "src", "screens", "Desk.svelte"), "utf8");

  it("the mockup ones are there: dot pulse, link flow, edge flash and cursor movement", () => {
    expect(css).toMatch(/\.on-dot \{[^}]*animation: pl 2s ease-in-out infinite/);
    expect(css).toMatch(/\.flow \{[^}]*animation: fl 1s linear infinite/);
    expect(css).toMatch(/\.flow\.r \{[^}]*animation-direction: reverse/);
    expect(css).toMatch(/\.edge\.on \{[^}]*animation: eg 1s ease-in-out/);
    expect(css).toMatch(/@keyframes eg \{/);
    expect(css).toMatch(/\.curdot \{[^}]*transition:/);
    expect(css).toMatch(/\.scene \{[^}]*transition:[^}]*border-color 0\.25s/);
    expect(css).toMatch(/\.thumb div \{[^}]*transition: background 0\.5s/);
  });

  it("with prefers-reduced-motion the demo does not move the cursor: it sends only the starting point", async () => {
    vi.useFakeTimers();
    setReducedMotion(true);
    const src = new DemoSource(demoOptions("demo"));
    await src.start(() => {});
    const seen: Notice[] = [];
    const stop = await src.watchCursor((e) => seen.push(e));
    await vi.advanceTimersByTimeAsync(5000);
    expect(seen).toHaveLength(1);
    stop();
  });

  it("without that preference the demo moves the cursor at about 30 Hz and announces the crossings", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    await src.start(() => {});
    const seen: Notice[] = [];
    const stop = await src.watchCursor((e) => seen.push(e));
    await vi.advanceTimersByTimeAsync(7000);
    stop();
    const moves = seen.filter((e) => e.kind === "cursor").length;
    expect(moves).toBeGreaterThan(150);
    expect(moves).toBeLessThan(230);
    const crossed = seen.filter((e) => e.kind === "crossed");
    expect(crossed.length).toBeGreaterThanOrEqual(2);
    expect(crossed[0]).toMatchObject({ from: "a", to: "b", edge: "right" });
  });
});

describe("live cursor (T-140)", () => {
  it("watchCursor is called only once even if the demo publishes a new snapshot on every tick", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    const spy = vi.spyOn(src, "watchCursor");
    const store = new Store(src);
    await store.start();
    render(Desk, { context: new Map([[STORE_KEY, store]]) });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("in 7 s the dot reaches the 34\" and the edge flashes on crossing, with no restarts", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    const store = new Store(src);
    await store.start();
    const { container } = render(Desk, { context: new Map([[STORE_KEY, store]]) });
    let max = 0;
    let flashed = false;
    for (let i = 0; i < 70; i++) {
      await vi.advanceTimersByTimeAsync(100);
      max = Math.max(max, parseFloat(dot(container)?.style.left ?? "0"));
      flashed ||= container.querySelector(".edge.on") !== null;
    }
    expect(max).toBeGreaterThanOrEqual(60);
    expect(flashed).toBe(true);
  });
});
