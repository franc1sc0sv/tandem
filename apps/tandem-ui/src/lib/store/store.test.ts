import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../bridge";
import type { Notice, SceneJson, Snapshot, TxnView } from "../model";
import { exitSide } from "../arrangement";
import { formatHash, parseHash, SCREEN_TOPICS } from "../routes";
import { DEMO_NOW, DemoSource, demoDesk, demoOptions, isDemo } from "./demo";
import { deskFromSnapshot, layoutFromDesk, LiveSource } from "./live";
import { SHELL_TOPICS, Store, TOAST_MS, TRAY_TOPICS, topicsFor } from "./store.svelte";

afterEach(() => {
  vi.useRealTimers();
});

describe("routes", () => {
  it("each screen has its address and comes back the same", () => {
    for (const hash of ["#/desk", "#/tunnels", "#/monitors", "#/arrangement", "#/network", "#/settings", "#/machine/a", "#/machine/b"]) {
      expect(formatHash(parseHash(hash))).toBe(hash);
    }
    expect(parseHash("")).toEqual({ screen: "desk" });
    expect(parseHash("#/machine/c")).toEqual({ screen: "desk" });
    expect(parseHash("#/nothing")).toEqual({ screen: "desk" });
  });
});

describe("store", () => {
  it("subscribes to what the shell always shows plus that of the visible screen", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    const set = (t: string[] | undefined) => new Set(t);
    expect(set(src.watched.at(-1))).toEqual(new Set([...SHELL_TOPICS, ...SCREEN_TOPICS.desk]));
    // The sidebar tunnel counter stays live on any screen.
    store.go({ screen: "network" });
    expect(set(src.watched.at(-1))).toEqual(new Set(["machines", "tunnels", "link", "network"]));
    store.go({ screen: "tunnels" });
    expect(set(src.watched.at(-1))).toEqual(new Set(["machines", "tunnels"]));
    for (const screen of ["desk", "tunnels", "monitors", "arrangement", "network", "settings"] as const) {
      expect(topicsFor({ screen })).toEqual(expect.arrayContaining(SHELL_TOPICS));
      expect(new Set(topicsFor({ screen })).size).toBe(topicsFor({ screen }).length);
    }
    // The other machine on the same screen does not subscribe again.
    store.go({ screen: "machine", machine: "a" });
    const n = src.watched.length;
    store.go({ screen: "machine", machine: "b" });
    expect(src.watched.length).toBe(n);
    expect(store.route).toEqual({ screen: "machine", machine: "b" });
  });

  it("resources only move if the screen shows them", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    const store = new Store(src);
    await store.start();
    const before = store.desk.machines.a?.resources?.history.join(",");
    vi.advanceTimersByTime(1600);
    expect(store.desk.machines.a?.resources?.history.join(",")).not.toBe(before);
    store.go({ screen: "network" });
    const frozen = store.desk.machines.a?.resources?.history.join(",");
    vi.advanceTimersByTime(5000);
    expect(store.desk.machines.a?.resources?.history.join(",")).toBe(frozen);
    store.stop();
  });

  it("the address changes and follows it", async () => {
    const loc = { hash: "#/network" };
    const store = new Store(new DemoSource(demoOptions("demo&still")), loc);
    expect(store.route).toEqual({ screen: "network" });
    store.go({ screen: "machine", machine: "b" });
    expect(loc.hash).toBe("#/machine/b");
    loc.hash = "#/arrangement";
    store.syncFromLocation();
    expect(store.route).toEqual({ screen: "arrangement" });
  });

  it("the actions announce what happened", async () => {
    vi.useFakeTimers();
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    await store.applyScene("game");
    expect(store.desk.activeScene).toBe("game");
    expect(store.toasts[0]).toMatchObject({ text: "Scene “Game” active.", tone: "ok" });
    await store.applyScene("nothing");
    expect(store.toasts[0]).toMatchObject({ tone: "bad" });
    vi.advanceTimersByTime(TOAST_MS);
    expect(store.toasts).toEqual([]);
  });

  it("power on, power off, measure and modules in the demo", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    await store.setPower("b", false);
    expect(store.desk.machines.b?.power).toBe("off");
    expect(store.desk.machines.b?.resources).toBeNull();
    await store.setPower("b", true);
    expect(store.desk.machines.b?.power).toBe("on");
    expect((await store.setPower("a", false)).ok).toBe(false);
    expect((await store.measureLink()).ok).toBe(true);
    await store.setModule("b", "clipboard", false);
    expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("off");
  });

  it("the tunnel counter counts the active ones", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    expect(store.activeTunnels).toBe(3);
  });

  it("the palette and the setup open and close", () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    store.togglePalette();
    expect(store.paletteOpen).toBe(true);
    store.openWizard();
    expect(store.paletteOpen).toBe(false);
    expect(store.wizardOpen).toBe(true);
    store.togglePalette(true);
    store.go({ screen: "network" });
    expect(store.paletteOpen).toBe(false);
  });

  it("if the source fails, the error is shown", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    src.applyScene = () => Promise.reject(new Error("it crashed"));
    const store = new Store(src);
    const r = await store.applyScene("x");
    expect(r).toEqual({ ok: false, reason: "it crashed" });
  });
});

describe("tray", () => {
  it("asks for its own set of topics and does not change with the route", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src, null, TRAY_TOPICS);
    await store.start();
    expect(src.watched).toEqual([TRAY_TOPICS]);
    store.go({ screen: "network" });
    expect(src.watched).toEqual([TRAY_TOPICS]);
    expect(TRAY_TOPICS).toEqual(expect.arrayContaining(["machines", "link", "scenes", "tunnels", "resources"]));
  });
});

describe("agent errors", () => {
  it("a snapshot with missing fields gives \"no agent\" with the reason, not \"connecting\"", async () => {
    const broken = { kind: "connected", agent: "a", desk: { screens: [], arrangement: { a_side: "left", offset: 0 } } };
    const bridge = { snapshot: async () => broken, onSnapshot: async () => {} } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    expect(store.desk.ready).toBe(true);
    expect(store.desk.trouble).toBe("no-agent");
    expect(store.desk.troubleText).toContain("does not understand");
  });

  it("an agent of the previous version (mac/pc sides) too", () => {
    const old = { kind: "connected", agent: "mac", desk: { screens: [], arrangement: { mac_side: "left", offset: 0 } } };
    expect(() => deskFromSnapshot(old as unknown as Snapshot)).toThrow(/side “mac”/);
  });

  it("if the source does not start, the error is shown", async () => {
    const bridge = { snapshot: async () => Promise.reject(new Error("no core")), onSnapshot: async () => {} } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    expect(store.desk).toMatchObject({ ready: true, trouble: "no-agent", troubleText: "no core" });
  });

  it("a broken snapshot after starting does not take down the window", async () => {
    let push: ((s: unknown) => void) | undefined;
    const ok = { kind: "unavailable", endpoint: null, reason: "x" };
    const bridge = { snapshot: async () => ok, onSnapshot: async (cb: (s: unknown) => void) => (push = cb) } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    push?.({ kind: "connected" });
    expect(store.desk.trouble).toBe("no-agent");
    expect(store.desk.troubleText).toContain("does not understand");
  });
});

describe("pairing and permissions", () => {
  it("the setup opens by itself on the pairing step when the code appears", async () => {
    vi.useFakeTimers();
    const store = new Store(new DemoSource(demoOptions("demo&still&peer=none")));
    await store.start();
    expect(store.wizardOpen).toBe(false);
    await store.pairAction("start_pairing");
    expect(store.wizardOpen).toBe(true);
    expect(store.wizardStep).toBe(1);
    vi.advanceTimersByTime(1500);
    expect(store.desk.pair.pairing).toMatchObject({ kind: "comparing", code: "418027" });
    await store.pairAction("confirm_pairing");
    vi.advanceTimersByTime(1300);
    expect(store.desk.pair.peer).toMatchObject({ kind: "paired", name: "PC-Desktop" });
    store.stop();
  });

  it("the tray does not open the setup", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still&peer=none")), null, TRAY_TOPICS);
    await store.start();
    await store.pairAction("start_pairing");
    expect(store.wizardOpen).toBe(false);
    store.stop();
  });

  it("first run: on macOS it opens on permissions; on Windows, without a peer, it opens on \"Your desk\"", async () => {
    const mac = new Store(new DemoSource(demoOptions("demo&still&setup")));
    await mac.start();
    await mac.maybeFirstRun();
    expect([mac.wizardOpen, mac.wizardStep]).toEqual([true, 2]);
    const win = new Store(new DemoSource(demoOptions("demo&still&setup&here=b")));
    await win.start();
    await win.maybeFirstRun();
    // With pairing already done there is nothing to offer.
    expect(win.wizardOpen).toBe(false);
    const fresh = new Store(new DemoSource(demoOptions("demo&still&setup&here=b&peer=none&demo=win-win")));
    await fresh.start();
    await fresh.maybeFirstRun();
    expect([fresh.wizardOpen, fresh.wizardStep]).toEqual([true, 1]);
    // Once finished, it does not open by itself again.
    await fresh.finishSetup();
    fresh.wizardOpen = false;
    await fresh.maybeFirstRun();
    expect(fresh.wizardOpen).toBe(false);
  });

  it("the real source passes the requests to the agent and reads the macOS permissions", async () => {
    const act = vi.fn(async () => ({ kind: "done" as const }));
    const openSettings = vi.fn(async () => {});
    const snap = {
      kind: "connected",
      agent: "a",
      desk: { machines: { a: null, b: null }, screens: [], arrangement: null, scenes: [] },
      state: { on: { a: true, b: false } },
      peer: { kind: "unpaired" },
      pairing: { kind: "idle" },
      monitors: [],
      input: { server: "a", hub_here: null, session: false, locked: false, missing: ["accessibility"], problem: null, defined: false },
      caps: {
        here: {
          machine: "a",
          name: "Studio",
          os: "macos",
          color: "#7aa7e6",
          hub: null,
          input: { capture: false, inject: false, missing: ["accessibility"], process: { exe: "/x/tandemd", pid: 7, started: 1 } },
          power: { wake_peer: true, wakeable: true, power_off: false },
          services: [],
        },
        peer: null,
      },
    };
    const bridge = { snapshot: async () => snap, onSnapshot: async () => {}, act, openSettings } as unknown as Bridge;
    const src = new LiveSource(bridge);
    const store = new Store(src);
    await store.start();
    expect(store.desk.permissions.needed).toBe(true);
    expect(store.desk.permissions.rows.map((r) => `${r.id}:${r.state}`)).toEqual(["accessibility:missing", "input_monitoring:granted", "local_network:unknown"]);
    expect(store.desk.permissions.binary).toContain("/x/tandemd (pid 7)");
    await store.pairAction("start_pairing");
    expect(act).toHaveBeenLastCalledWith({ kind: "start_pairing" });
    await store.requestPermissions();
    expect(act).toHaveBeenLastCalledWith({ kind: "request_permissions" });
    await store.openPermission("accessibility");
    expect(openSettings).toHaveBeenCalledWith("accessibility");
    act.mockResolvedValueOnce({ kind: "rejected", reason: "one is already in progress" } as never);
    expect(await store.pairAction("start_pairing")).toEqual({ ok: false, reason: "one is already in progress" });
  });
});

describe("demo mode", () => {
  it("is requested with ?demo", () => {
    expect(isDemo("?demo")).toBe(true);
    expect(isDemo("?demo=win-win&still")).toBe(true);
    expect(isDemo("?x=1")).toBe(false);
  });

  it("is deterministic and coherent in each combination of systems (D2)", () => {
    for (const pair of ["mac-win", "win-win", "mac-mac"] as const) {
      const d = demoDesk({ pair, here: "a", trouble: "none", still: true });
      expect(d).toEqual(demoDesk({ pair, here: "a", trouble: "none", still: true }));
      expect(d.machines.a?.here).toBe(true);
      expect(d.machines.b?.here).toBe(false);
      expect(d.machines.a?.name).not.toBe(d.machines.b?.name);
      expect(d.machines.a?.color).not.toBe(d.machines.b?.color);
    }
    const ww = demoDesk(demoOptions("demo=win-win"));
    expect([ww.machines.a?.os, ww.machines.b?.os]).toEqual(["windows", "windows"]);
    const mm = demoDesk(demoOptions("demo=mac-mac&here=b"));
    expect([mm.machines.a?.os, mm.machines.b?.os]).toEqual(["macos", "macos"]);
    expect(mm.here).toBe("b");
  });

  it("each trouble state", () => {
    const off = demoDesk(demoOptions("demo&trouble=peer-off"));
    expect(off.trouble).toBe("peer-off");
    expect(off.troubleText).toBe("PC-Desktop is off");
    expect(off.machines.b?.power).toBe("off");
    expect(off.link.kind).toBeNull();
    const down = demoDesk(demoOptions("demo&trouble=link-down"));
    expect(down.machines.b?.power).toBe("lost");
    const perms = demoDesk(demoOptions("demo&trouble=permissions"));
    expect(perms.permissions.rows.filter((r) => r.state === "missing").map((r) => r.id)).toEqual(["accessibility", "input_monitoring"]);
    expect(demoDesk(demoOptions("demo&trouble=no-agent")).troubleText).toContain("agent");
    expect(demoOptions("demo&trouble=other").trouble).toBe("none");
  });
});

describe("network en el demo (T-102)", () => {
  it("the paths of V3-Network: primary, no cable and backup, with 60 s and the switch to Wi-Fi", () => {
    const n = demoDesk(demoOptions("demo&still")).network;
    expect(n.paths.map((p) => [p.kind, p.state])).toEqual([
      ["usb4", "primary"],
      ["ethernet", "unplugged"],
      ["wifi", "backup"],
    ]);
    const usb = n.paths[0]!;
    expect([usb.rttMs, usb.inputP50Ms, usb.inputP99Ms, usb.targetMs, usb.gbps]).toEqual([0.3, 0.4, 1.9, 4, 9.4]);
    expect(n.paths.every((p) => p.history.length === 60)).toBe(true);
    expect(n.paths[2]?.targetMs).toBe(20);
    expect(n.switches[1]).toMatchObject({ from: "usb4", to: "wifi", tookMs: 600 });
    expect(n.switches[0]?.at).toBeGreaterThan(n.switches[1]!.at);
    // The chart peak belongs to Wi-Fi, while the cable was out (cut 36 s ago).
    const wifi = n.paths[2]!.history;
    const peak = Math.max(...wifi.map((h) => h.inputP99Ms ?? 0));
    expect(peak).toBe(5.6);
    expect(wifi.findIndex((h) => h.inputP99Ms === peak)).toBe(27);
    expect(DEMO_NOW - n.switches[1]!.at).toBe(36_000);
    expect(n.now).toBe(DEMO_NOW);
    expect(n.paths.map((p) => p.detail)).toEqual(["USB4 · 20 Gbps", "2.5 GbE", "Wi-Fi 7 · home"]);
    // No link, all no cable.
    const off = demoDesk(demoOptions("demo&trouble=peer-off")).network;
    expect(off.paths.every((p) => p.state === "unplugged" && p.history.length === 0)).toBe(true);
  });

  it("the history only advances with the Network screen visible", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    const store = new Store(src);
    await store.start();
    const usb = () => store.desk.network.paths[0]!.history.map((h) => h.rttMs).join(",");
    const before = usb();
    vi.advanceTimersByTime(5000);
    expect(usb()).toBe(before);
    store.go({ screen: "network" });
    vi.advanceTimersByTime(1600);
    expect(usb()).not.toBe(before);
    expect(store.desk.network.paths[0]!.history).toHaveLength(60);
    // The one that does not respond does not advance.
    expect(store.desk.network.paths[1]!.history.every((h) => h.rttMs === null)).toBe(true);
  });

  it("measuring any path that responds", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    expect(await src.measureLink("wifi")).toEqual({ ok: true, text: "Wi-Fi: 410 Mbps" });
    expect(store.desk.network.paths[2]?.gbps).toBe(0.41);
    expect(await src.measureLink()).toEqual({ ok: true, text: "USB-C: 9.4 Gbps" });
    expect((await src.measureLink("ethernet")).ok).toBe(false);
  });
});

describe("crossing test in demo mode", () => {
  it("\"Test crossing\" is available and the crossing leaves through the arrangement edge (and the wrong one with cross=wrong)", async () => {
    for (const [opts, same] of [["demo&cross=ok", true], ["demo&cross=wrong", false]] as const) {
      const src = new DemoSource(demoOptions(opts));
      const store = new Store(src);
      await store.start();
      expect(src.can("crossing")).toBe(true);
      const got = await new Promise<{ edge: string }>((resolve) => {
        void store.watchCrossing((s) => resolve(s));
      });
      const { arrangement } = store.desk.layout;
      const expected = exitSide(arrangement, store.desk.here);
      expect(got.edge === expected).toBe(same);
    }
  });
});

describe("real source", () => {
  const connected = (over: Partial<Extract<Snapshot, { kind: "connected" }>> = {}): Snapshot => ({
    kind: "connected",
    agent: "b",
    desk: { machines: { a: { os: "windows", name: "Workshop", color: "#6cc59a" }, b: null }, screens: [], arrangement: { a_side: "up", align: "end" }, scenes: [] },
    state: { on: { a: true, b: true } },
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
    monitors: [],
    ...over,
  });

  it("translates the agent snapshot: names, colors and link of each side", () => {
    const d = deskFromSnapshot(connected());
    expect(d.ready).toBe(true);
    expect(d.here).toBe("b");
    expect(d.machines.b).toMatchObject({ name: "Studio", color: "#d9a75a", here: true, power: "on", detail: "Windows" });
    expect(d.machines.a).toMatchObject({ name: "Workshop", color: "#6cc59a", here: false });
    expect(d.link.kind).toBe("wifi");
    expect(d.trouble).toBe("none");
  });

  it("without [[screens]], the arrangement uses the screens the engine of each machine reads, at scale between them", () => {
    const snap = connected({
      agent: "a",
      input: {
        server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true,
        screens: [{ id: "m32", x: 0, y: 0, width: 1920, height: 1080, scale_milli: 1000 }],
        peer_screens: [{ id: "m34", x: 0, y: 0, width: 3440, height: 1440, scale_milli: 1500 }],
      },
    });
    if (snap.kind !== "connected") throw new Error("snapshot");
    const { screens } = layoutFromDesk(snap);
    const a = screens.find((s) => s.machine === "a");
    const b = screens.find((s) => s.machine === "b");
    expect(a).toMatchObject({ res: "1920 × 1080", ratio: "16:9", scale: "100 %" });
    expect(b).toMatchObject({ res: "3440 × 1440", ratio: "21:9", scale: "150 %" });
    // 3440 px at 150 % is 2293 points: the drawing respects that proportion between the two machines.
    expect(b!.w / a!.w).toBeCloseTo(3440 / 1.5 / 1920, 3);
    expect(a!.native).toMatchObject({ id: "m32", w: 1920 });
  });

  it("T-340: Live desktops draws the shared 34\" once, on the machine it shows", () => {
    const m34 = { id: "m34", x: 0, y: 0, width: 3440, height: 1440, scale_milli: 1000 };
    const report = (input: "display_port" | "hdmi") => ({ id: "lc34", input, power_on: true, signal: true, events: [] });
    const snap = (input: "display_port" | "hdmi") => {
      const s = connected({
        agent: "b",
        desk: {
          machines: { a: { os: "windows", name: "PC", color: "#6cc59a" }, b: { os: "macos", name: "Mac", color: "#d9a75a" } },
          screens: [],
          arrangement: null,
          scenes: [],
          cables: [{ monitor: "lc34", a: { input: "display_port", screen: "screen-1" }, b: { input: "hdmi", screen: "screen-2" } }],
        },
        input: {
          server: "b", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true,
          screens: [{ id: "screen-1", x: 0, y: 0, width: 1512, height: 982, scale_milli: 1000 }, { ...m34, id: "screen-2", x: 1512 }],
          peer_screens: [{ ...m34, id: "screen-1" }],
        },
        monitors: [{
          id: "lc34", name: "LC34G55T", compat: { Compatible: { protocol: 2 } }, seen_by: ["a", "b"],
          via: [["a", "display_port"], ["b", "hdmi"]], owner: "a", report: report(input),
        }],
      });
      if (s.kind !== "connected") throw new Error("snapshot");
      return s;
    };
    const ids = (input: "display_port" | "hdmi") => layoutFromDesk(snap(input)).screens.map((s) => s.id);
    // On the PC (DP): the Mac's HDMI screen is not drawn.
    expect(ids("display_port")).toEqual(["a:screen-1", "b:screen-1"]);
    // On the Mac (HDMI): the PC has no other screen, so it draws nothing.
    expect(ids("hdmi")).toEqual(["b:screen-1", "b:screen-2"]);
  });

  it("T-356: a cabled monitor is drawn at its EDID size, named by it and with its model's stand", () => {
    const shows = { state: { shows_machine: "b" as const }, power: "on" as const, cables: { a: "connected" as const, b: "connected" as const } };
    const snap = connected({
      agent: "b",
      desk: {
        machines: { a: { os: "windows", name: "DESKTOP-7Q4K2M9", color: "#a99be8" }, b: { os: "macos", name: "Mac", color: "#7aa7e6" } },
        screens: [],
        arrangement: null,
        scenes: [],
        cables: [
          { monitor: "lc34", a: { input: "display_port", screen: "screen-1" }, b: { input: "hdmi", screen: "screen-1" } },
          { monitor: "g5", a: { input: "display_port", screen: "screen-2" }, b: { input: "hdmi", screen: "screen-2" } },
        ],
      },
      input: {
        server: "b", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true,
        // macOS points: the ultrawide at 1:1 and the G5 scaled; the engine reads them side by side.
        screens: [{ id: "screen-1", x: 0, y: 0, width: 3440, height: 1440, scale_milli: 1000 }, { id: "screen-2", x: 3440, y: 0, width: 1920, height: 1080, scale_milli: 1333 }],
        peer_screens: [{ id: "screen-1", x: 0, y: 0, width: 3440, height: 1440, scale_milli: 1000 }, { id: "screen-2", x: 3440, y: 0, width: 2560, height: 1440, scale_milli: 1000 }],
      },
      monitors: [
        { id: "lc34", name: "LC34G55T", compat: { Compatible: { protocol: 3 } }, seen_by: ["a", "b"], via: [], owner: "a", report: null, link: shows, size_mm: [797, 334] },
        { id: "g5", name: "Odyssey G5", compat: { Compatible: { protocol: 3 } }, seen_by: ["a", "b"], via: [], owner: "a", report: null, link: shows, size_mm: [700, 400] },
      ],
    });
    if (snap.kind !== "connected") throw new Error("snapshot");
    const { screens } = layoutFromDesk(snap);
    // Both show the Mac: the PC draws nothing.
    expect(screens.map((s) => s.id)).toEqual(["b:screen-1", "b:screen-2"]);
    const [wide, g5] = screens;
    expect(wide).toMatchObject({ name: "LC34G55T", short: '34"', size: '34"', ask: 'the 34"', external: true, stand: "ultrawide-g5" });
    expect(g5).toMatchObject({ name: "Odyssey G5", short: '32"', size: '32"', external: true, stand: "g5" });
    expect(wide!.w).toBeCloseTo(797 / 25.4, 6);
    expect(g5!.h).toBeCloseTo(400 / 25.4, 6);
    // Touching at their real widths, even though the G5 has another density.
    expect(g5!.x - wide!.x).toBeCloseTo(797 / 25.4, 6);
    // A monitor without a size keeps the engine's estimate, named by the EDID model.
    const noSize = { ...snap, monitors: snap.monitors.map((m) => ({ ...m, size_mm: null })) };
    const [w2] = layoutFromDesk(noSize).screens;
    expect(w2).toMatchObject({ name: "LC34G55T", short: "LC34G55T", size: "—", stand: "ultrawide-g5" });
    expect(w2!.w).toBeCloseTo(3440 / 110, 6);
  });

  it("watchCrossing delivers the real Crossed with the input measured on the arrival screen", async () => {
    const act = vi.fn(async (_a: unknown) => ({ kind: "done" as const }));
    let notice: ((n: Notice) => void) | undefined;
    const bridge = {
      snapshot: async () => connected({
        agent: "a",
        input: {
          server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true,
          screens: [{ id: "m32", x: 0, y: 0, width: 1920, height: 1080, scale_milli: 1000 }],
          peer_screens: [{ id: "m34", x: 0, y: 0, width: 3440, height: 1440, scale_milli: 1000 }],
        },
      }),
      onSnapshot: async () => {},
      act,
      onNotice: async (cb: (n: Notice) => void) => {
        notice = cb;
      },
    } as unknown as Bridge;
    const src = new LiveSource(bridge);
    await src.start(() => {});
    expect(src.can("crossing")).toBe(true);
    const got: unknown[] = [];
    const stop = await src.watchCrossing((s) => got.push(s));
    expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true });
    notice?.({ kind: "crossed", from: "a", to: "b", edge: "right" });
    notice?.({ kind: "cursor", sample: { machine: "b", screen: "m34", x: 2, y: 600 } });
    expect(got).toEqual([expect.objectContaining({ from: "a", to: "b", edge: "right", at: 600, of: 1440 })]);
    stop();
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_cursor", on: false });
  });

  describe("watchCrossing without an arrival sample (fake clock)", () => {
    async function started() {
      vi.useFakeTimers();
      let notice: ((n: Notice) => void) | undefined;
      const bridge = {
        snapshot: async () => connected({ agent: "a" }),
        onSnapshot: async () => {},
        act: async () => ({ kind: "done" as const }),
        onNotice: async (cb: (n: Notice) => void) => {
          notice = cb;
        },
      } as unknown as Bridge;
      const src = new LiveSource(bridge);
      await src.start(() => {});
      const got: { from: string; edge: string; at: number; of: number; ms: number | null }[] = [];
      await src.watchCrossing((s) => got.push(s));
      return { got, notice: (n: Notice) => notice?.(n) };
    }
    afterEach(() => vi.useRealTimers());

    it("a crossed without a cursor is reported at 250 ms with of 0 and without an invented latency", async () => {
      const { got, notice } = await started();
      notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
      vi.advanceTimersByTime(249);
      expect(got).toEqual([]);
      vi.advanceTimersByTime(2);
      expect(got).toEqual([expect.objectContaining({ from: "a", edge: "left", at: 0, of: 0, ms: null })]);
    });

    it("a new crossed replaces the pending one: only the last is reported", async () => {
      const { got, notice } = await started();
      notice({ kind: "crossed", from: "a", to: "b", edge: "up" });
      vi.advanceTimersByTime(200);
      notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
      vi.advanceTimersByTime(200);
      expect(got).toEqual([]);
      vi.advanceTimersByTime(100);
      expect(got).toEqual([expect.objectContaining({ edge: "left", ms: null })]);
    });

    it("a sample from another machine does not close the pending crossing", async () => {
      const { got, notice } = await started();
      notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
      notice({ kind: "cursor", sample: { machine: "a", screen: "m32", x: 5, y: 5 } });
      expect(got).toEqual([]);
      vi.advanceTimersByTime(300);
      expect(got).toHaveLength(1);
    });
  });

  it("the trouble states come from the snapshot", () => {
    expect(deskFromSnapshot({ kind: "connecting" }).ready).toBe(false);
    expect(deskFromSnapshot({ kind: "unavailable", endpoint: null, reason: "not running" })).toMatchObject({ trouble: "no-agent", troubleText: "not running" });
    expect(deskFromSnapshot(connected({ state: { on: { a: false, b: true } } }))).toMatchObject({ trouble: "peer-off", troubleText: "Workshop is off" });
    expect(deskFromSnapshot(connected({ peer: { kind: "paired", name: "Workshop", link: null } })).trouble).toBe("link-down");
    const missing = connected();
    if (missing.kind === "connected") missing.input = { ...missing.input, missing: ["accessibility"] };
    expect(deskFromSnapshot(missing).trouble).toBe("permissions");
  });

  it("T-159: the desk name and the MAC to power on come from the snapshot", () => {
    const snap = connected();
    expect(deskFromSnapshot(snap).deskName).toBe("My desk");
    expect(deskFromSnapshot(snap).machines.a?.wake).toEqual({ mac: null });
    if (snap.kind === "connected") {
      snap.desk = { ...snap.desk, desk_name: " Office " };
      snap.caps.here.power = { ...snap.caps.here.power, wol_mac: "aa:bb:cc:dd:ee:ff" };
    }
    const d = deskFromSnapshot(snap);
    expect(d.deskName).toBe("Office");
    expect(d.machines.a?.wake).toEqual({ mac: "aa:bb:cc:dd:ee:ff" });
    expect(d.machines.b?.wake).toBeNull();
  });

  it("asks the agent for what it can do and says what it cannot yet", async () => {
    const act = vi.fn(async (_a: unknown) => ({ kind: "bandwidth" as const, link: "wifi" as const, bytes_per_sec: 51_250_000 }));
    let push: ((s: Snapshot) => void) | undefined;
    const bridge = {
      snapshot: async () => connected(),
      onSnapshot: async (cb: (s: Snapshot) => void) => {
        push = cb;
      },
      act,
    } as unknown as Bridge;
    const src = new LiveSource(bridge);
    const store = new Store(src);
    await store.start();
    expect(store.desk.machines.b?.name).toBe("Studio");
    push?.(connected({ caps: { ...((connected() as Extract<Snapshot, { kind: "connected" }>).caps), here: { ...(connected() as Extract<Snapshot, { kind: "connected" }>).caps.here, name: "New" } } }));
    expect(store.desk.machines.b?.name).toBe("New");
    expect(src.can("measure")).toBe(true);
    // With the agent connected, scenes are applied and monitors changed (T-116).
    expect(src.can("scenes")).toBe(true);
    expect(src.can("monitors")).toBe(true);
    expect(src.can("forget")).toBe(false);
    // Without a network in sight, it measures the path the other machine is seen through (410 Mbps = 51.25 MB/s).
    expect(await src.measureLink()).toEqual({ ok: true, text: "Wi-Fi: 410 Mbps" });
    expect(act).toHaveBeenLastCalledWith({ kind: "measure_link", path: "wifi" });
    expect(await src.measureLink("usb4")).toEqual({ ok: true, text: "USB-C: 410 Mbps" });
    expect(act).toHaveBeenLastCalledWith({ kind: "measure_link", path: "usb4" });
    act.mockResolvedValueOnce({ kind: "accepted", txn: 2 } as never);
    expect(await src.applyScene("game")).toEqual({ ok: true, txn: 2 });
    expect(act).toHaveBeenLastCalledWith({ kind: "apply_scene", id: "game" });
    act.mockResolvedValueOnce({ kind: "rejected", reason: "hdmi no signal" } as never);
    expect(await src.switchMonitor("g34", "hdmi")).toEqual({ ok: false, reason: "hdmi no signal" });
    expect(act).toHaveBeenLastCalledWith({ kind: "switch_monitor", monitor: "g34", input: "hdmi" });
    act.mockResolvedValueOnce({ kind: "done" } as never);
    expect(await src.setModule("b", "clipboard", false)).toEqual({ ok: true });
    expect(act).toHaveBeenLastCalledWith({ kind: "set_module", machine: "b", module: "clipboard", on: false });
    act.mockResolvedValueOnce({ kind: "accepted", txn: 1 } as never);
    expect(await src.setPower("a", false)).toEqual({ ok: true });
    expect(act).toHaveBeenLastCalledWith({ kind: "set_power", machine: "a", on: false });
    src.watch(["link"]);
    expect(src.currentTopics).toEqual(["link"]);
  });
});

describe("scenes and transactions against the simulated bridge", () => {
  const scene: SceneJson = { id: "game", name: "Game", monitors: [["g34", "b"]], shortcut: ["ControlLeft", "AltLeft", "Digit4"] };

  function fake() {
    const act = vi.fn(async (_a: unknown) => ({ kind: "done" as const }));
    let notice: ((n: Notice) => void) | undefined;
    const onNotice = vi.fn(async (cb: (n: Notice) => void) => {
      notice = cb;
    });
    const bridge = { snapshot: async () => ({ kind: "connecting" }) as Snapshot, onSnapshot: async () => {}, act, onNotice } as unknown as Bridge;
    return { src: new LiveSource(bridge), act, onNotice, emit: (event: TxnView) => notice?.({ kind: "txn", event } as Notice) };
  }

  it("saveScene creates with add_scene and edits with edit_scene, and says the saved text", async () => {
    const { src, act } = fake();
    expect(await src.saveScene(scene, true)).toEqual({ ok: true, text: "Scene “Game” saved" });
    expect(act).toHaveBeenLastCalledWith({ kind: "add_scene", scene });
    expect(await src.saveScene(scene, false)).toEqual({ ok: true, text: "Scene “Game” saved" });
    expect(act).toHaveBeenLastCalledWith({ kind: "edit_scene", scene });
  });

  it("saveScene returns the reason if the agent rejects the scene", async () => {
    const { src, act } = fake();
    act.mockResolvedValueOnce({ kind: "rejected", reason: "the shortcut is already used by “Work”" } as never);
    expect(await src.saveScene(scene, true)).toEqual({ ok: false, reason: "the shortcut is already used by “Work”" });
  });

  it("watchTxn listens once, delivers each notice to all and stops delivering on cancel", async () => {
    const { src, onNotice, emit } = fake();
    const a: TxnView[] = [];
    const b: TxnView[] = [];
    const stopA = await src.watchTxn((e) => a.push(e));
    await src.watchTxn((e) => b.push(e));
    expect(onNotice).toHaveBeenCalledTimes(1);
    const started: TxnView = { type: "started", txn: 3, steps: [{ step: "geometry" }] };
    emit(started);
    expect(a).toEqual([started]);
    expect(b).toEqual([started]);
    stopA();
    emit({ type: "step_started", txn: 3, index: 0 });
    expect(a).toHaveLength(1);
    expect(b).toHaveLength(2);
  });

  it("the store splits power-on from the scene and remembers the measured link", async () => {
    const { src, emit, act } = fake();
    const store = new Store(src);
    await store.start();
    emit({ type: "started", txn: 1, steps: [{ step: "geometry" }] });
    emit({ type: "started", txn: 2, steps: [{ step: "power_on", machine: "b" }] });
    expect(store.run?.progress.txn).toBe(1);
    expect(store.powerRun?.progress.txn).toBe(2);
    emit({ type: "step_started", txn: 2, index: 0 });
    expect(store.powerRun?.progress.status[0]).toBe("run");
    expect(store.run?.progress.status[0]).toBe("pending");
    act.mockResolvedValueOnce({ kind: "bandwidth", link: "wifi", bytes_per_sec: 51_250_000 } as never);
    await store.measureLink("wifi");
    expect(store.measured).toBe("wifi");
  });
});

describe("the demo link without pairing", () => {
  it("before pairing the other machine is only seen over Wi-Fi; after pairing it goes over the cable", async () => {
    vi.useFakeTimers();
    const store = new Store(new DemoSource(demoOptions("demo&still&peer=none")));
    await store.start();
    expect(store.desk.link.kind).toBe("wifi");
    await store.pairAction("start_pairing");
    await vi.advanceTimersByTimeAsync(2000);
    await store.pairAction("confirm_pairing");
    await vi.advanceTimersByTimeAsync(5000);
    expect(store.desk.pair.peer.kind).toBe("paired");
    expect(store.desk.link.kind).toBe("usb4");
    await store.pairAction("unpair");
    expect(store.desk.link.kind).toBe("wifi");
  });
});
