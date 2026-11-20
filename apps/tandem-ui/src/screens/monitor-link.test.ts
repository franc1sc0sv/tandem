// T-349: the monitor link states on Desk, Monitors and the tray, against the real agent
// (simulated IPC), and the link notices, which never repeat a state.
import { render, screen, within } from "@testing-library/svelte";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import { DIM_WALL } from "../lib/desk";
import type { LinkJson, MonitorJson, Snapshot } from "../lib/model";
import { linkText, linkUi, monitorUi, whyNotSwitch } from "../lib/monitors";
import { monitorNotices } from "../lib/notices";
import { LiveSource } from "../lib/store/live";
import { Store, TRAY_TOPICS } from "../lib/store/store.svelte";
import Shell from "../shell/Shell.svelte";
import Tray from "../Tray.svelte";

const SHOWS_PC: LinkJson = { state: { shows_machine: "b" }, power: "on", cables: { a: "connected", b: "connected" } };
const UNPLUGGED: LinkJson = { state: { active_input_unplugged: "a" }, power: "standby", cables: { a: "disconnected", b: "connected" } };
const STANDBY: LinkJson = { state: "standby", power: "standby", cables: { a: "connected", b: "connected" } };
const OFF: LinkJson = { state: "off", power: "off", cables: { a: "connected", b: "connected" } };

const STATES: [string, LinkJson, string, boolean][] = [
  ["showing the PC", SHOWS_PC, "Showing PC", false],
  ["the Mac's input unplugged", UNPLUGGED, "Mac cable unplugged", true],
  ["in standby", STANDBY, "Monitor in standby", true],
  ["off", OFF, "Monitor off", true],
];

function monitor(link: LinkJson): MonitorJson {
  return {
    id: "lc34-1",
    name: 'Odyssey G5 34"',
    compat: { Compatible: { protocol: 2 } },
    seen_by: ["a", "b"],
    via: [
      ["a", "hdmi"],
      ["b", "display_port"],
    ],
    owner: "b",
    report: { id: "lc34-1", input: "display_port", power_on: true, signal: true, events: [] },
    link,
  };
}

function connected(link: LinkJson): Snapshot {
  return {
    kind: "connected",
    agent: "b",
    desk: {
      machines: { a: { os: "macos", name: "Mac", color: "#7aa7e6" }, b: { os: "windows", name: "PC", color: "#a99be8" } },
      screens: [],
      arrangement: { a_side: "left", align: "center" },
      scenes: [],
      cables: [{ monitor: "lc34-1", a: { input: "hdmi", screen: "screen-2" }, b: { input: "display_port", screen: "screen-1" } }],
    },
    state: { on: { a: true, b: true } },
    monitors: [monitor(link)],
    peer: { kind: "paired", name: "Mac", link: "wifi" },
    pairing: { kind: "idle" },
    input: {
      server: "b",
      hub_here: null,
      session: true,
      locked: false,
      missing: [],
      problem: null,
      defined: true,
      screens: [{ id: "screen-1", x: 0, y: 0, width: 3440, height: 1440, scale_milli: 1000 }],
      peer_screens: [
        { id: "screen-1", x: 0, y: 0, width: 1512, height: 982, scale_milli: 2000 },
        { id: "screen-2", x: 1512, y: 0, width: 3440, height: 1440, scale_milli: 1000 },
      ],
    },
    caps: {
      here: {
        machine: "b",
        name: "PC",
        os: "windows",
        color: "#a99be8",
        hub: null,
        input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: true },
        services: [],
      },
      peer: null,
    },
  } as unknown as Snapshot;
}

function bridgeOf(snap: Snapshot): Bridge {
  return { snapshot: async () => snap, onSnapshot: async () => {}, act: vi.fn(async () => ({ kind: "done" })), shortcuts: async () => ({ items: [], mac: false }) } as unknown as Bridge;
}

async function open(link: LinkJson, hash: string) {
  const store = new Store(new LiveSource(bridgeOf(connected(link))), { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const name = (m: "a" | "b") => (m === "a" ? "Mac" : "PC");

describe("monitor link states (T-349)", () => {
  it("each state has its plain words", () => {
    for (const [, link, text] of STATES) expect(linkText(linkUi(link), name)).toBe(text);
    expect(linkText(linkUi({ state: "unknown", power: "unknown", cables: { a: "unknown", b: "unknown" } }), name)).toBeNull();
    // An off monitor shows nobody, whatever was read last.
    expect(monitorUi(monitor(OFF)).shows).toBeNull();
    expect(monitorUi(monitor(SHOWS_PC)).shows).toBe("b");
  });

  it("the selector refuses a machine whose cable is unplugged, but not an unknown one", () => {
    expect(whyNotSwitch(monitorUi(monitor(UNPLUGGED)), "a", "Mac")).toMatch(/Mac cable unplugged/);
    const unknown = monitor({ ...SHOWS_PC, cables: { a: "unknown", b: "connected" } });
    expect(whyNotSwitch(monitorUi(unknown), "a", "Mac")).toBeNull();
  });

  for (const [what, link, text, dim] of STATES) {
    it(`Desk: ${what} says "${text}" and draws the live desktop ${dim ? "dimmed" : "of the machine shown"}`, async () => {
      const store = await open(link, "#/desk");
      expect(screen.getByRole("img", { name: new RegExp(`34": ${text}\\.`) })).toBeInTheDocument();
      const shown = store.desk.layout.screens.filter((s) => s.native?.id === "screen-2" || s.native?.id === "screen-1");
      if (dim) {
        expect(screen.getAllByText(`34" · ${text}`).length).toBeGreaterThan(0);
        const dimmed = document.querySelectorAll(".desk.dim");
        expect(dimmed).toHaveLength(1);
        expect((dimmed[0]?.querySelector(".w") as HTMLElement).style.background).toBe("rgb(22, 23, 27)");
        // Drawn once, as neither machine's screen.
        expect(shown.filter((s) => s.dim)).toHaveLength(1);
        expect(DIM_WALL).toBe("#16171b");
      } else {
        expect(document.querySelectorAll(".desk.dim")).toHaveLength(0);
        expect(shown.every((s) => !s.dim)).toBe(true);
      }
    });

    it(`Monitors: ${what} says "${text}"${dim ? " with a dimmed thumbnail" : ""}`, async () => {
      await open(link, "#/monitors");
      const card = screen.getByRole("article", { name: 'Odyssey G5 34"' });
      const thumb = within(card).getByRole("img", { name: text });
      expect(thumb.classList.contains("dim")).toBe(dim);
      if (dim) expect(within(card).getByText(text)).toBeInTheDocument();
    });
  }

  it("the tray says the state of a monitor that shows neither machine", async () => {
    const store = new Store(new LiveSource(bridgeOf(connected(UNPLUGGED))), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText('34" · Mac cable unplugged')).toBeInTheDocument());
  });

  it("the tray has no monitor line while it shows a machine", async () => {
    const store = new Store(new LiveSource(bridgeOf(connected(SHOWS_PC))), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("Mac")).toBeInTheDocument());
    expect(screen.queryByText(/34" ·/)).toBeNull();
  });
});

describe("link notices (T-349)", () => {
  const ui = (link: LinkJson) => [monitorUi(monitor(link))];

  it("transitions are announced once, and an unknown in between never repeats one", () => {
    let memo = {};
    const step = (link: LinkJson) => {
      const r = monitorNotices(memo, ui(link), name);
      memo = r.memo;
      return r.specs.map((s) => s.text);
    };
    expect(step(SHOWS_PC)).toEqual([]);
    expect(step(SHOWS_PC)).toEqual([]);
    expect(step(UNPLUGGED)).toEqual(["Mac cable unplugged"]);
    expect(step(UNPLUGGED)).toEqual([]);
    // The Mac's end becomes unknown, then disconnected again: not announced again.
    expect(step({ ...UNPLUGGED, cables: { a: "unknown", b: "connected" } })).toEqual([]);
    expect(step(UNPLUGGED)).toEqual([]);
    expect(step(SHOWS_PC)).toEqual(["Mac cable connected"]);
    expect(step(OFF)).toEqual(['34" turned off']);
    expect(step(OFF)).toEqual([]);
    expect(step({ state: "unknown", power: "unknown", cables: { a: "connected", b: "connected" } })).toEqual([]);
    expect(step(OFF)).toEqual([]);
    expect(step(SHOWS_PC)).toEqual(['34" back on']);
  });

  it("the first state seen is remembered without a notice", () => {
    expect(monitorNotices({}, ui(OFF), name).specs).toEqual([]);
    expect(monitorNotices({}, ui(UNPLUGGED), name).specs).toEqual([]);
  });
});

describe("demo mode (`?demo&monitor=`, T-349)", () => {
  for (const [state, text] of [
    ["off", "Monitor off"],
    ["standby", "Monitor in standby"],
    ["unplugged", "PC-Desktop cable unplugged"],
  ] as const) {
    it(`monitor=${state} shows "${text}" dimmed on Monitors and Desk`, async () => {
      const { DemoSource, demoOptions } = await import("../lib/store/demo");
      const store = new Store(new DemoSource(demoOptions(`demo&still&monitor=${state}`)), { hash: "#/monitors" });
      await store.start();
      render(Shell, { context: new Map([[STORE_KEY, store]]) });
      await tick();
      const card = screen.getByRole("article", { name: 'Odyssey G5 34"' });
      expect(within(card).getByRole("img", { name: text }).classList.contains("dim")).toBe(true);
      expect(store.desk.layout.screens.filter((s) => s.dim === text)).toHaveLength(1);
    });
  }
});
