import { describe, expect, it } from "vitest";

import { start } from "./crossing";
import type { Side } from "./model";
import { DemoSource, demoOptions } from "./store/demo";
import {
  NO_ACTIVITY,
  SKETCH,
  WIZARD_STEPS,
  arrowSide,
  clampStep,
  computeNote,
  crossVerdict,
  computeWorks,
  deviceRows,
  devicesOfHub,
  dropSide,
  foundRows,
  serviceRow,
  heard,
  missingOf,
  permCards,
  permFooter,
  stepMark,
  summaryRows,
  systemNote,
} from "./wizard";

async function desk(query = "demo&still") {
  let d = null as Awaited<ReturnType<typeof current>> | null;
  d = await current(query);
  return d;
}
async function current(query: string) {
  const s = new DemoSource(demoOptions(query));
  let out: Parameters<Parameters<DemoSource["start"]>[0]>[0] | null = null;
  await s.start((v) => (out = v));
  if (!out) throw new Error("no state");
  return out as Parameters<Parameters<DemoSource["start"]>[0]>[0];
}

describe("steps", () => {
  it("there are six and each is done, current or to do", () => {
    expect(WIZARD_STEPS).toHaveLength(6);
    expect([0, 1, 2, 3, 4, 5].map((i) => stepMark(i, 2))).toEqual(["done", "done", "cur", "todo", "todo", "todo"]);
  });

  it("clampStep never leaves the list", () => {
    for (let i = -50; i <= 50; i++) expect(clampStep(i)).toBeGreaterThanOrEqual(0), expect(clampStep(i)).toBeLessThanOrEqual(5);
  });
});

describe("step 1", () => {
  it("the system row composes system, processor and memory", async () => {
    const d = await desk();
    const mac = d.machines.a!;
    expect(systemNote(mac)).toBe("macOS 26 · Apple M5 · 24 GB");
    expect(computeNote(mac)).toBe("zsh · Docker · uv · node");
    const rows = foundRows(mac);
    expect(rows.map((r) => r.label)).toEqual(["macOS 26 · Apple M5 · 24 GB", "Keyboard and mouse", "Clipboard", "Compute", "Monitors", "Administrator service"]);
    expect(rows.find((r) => r.id === "monitors")).toMatchObject({ state: "off", note: "no DDC channel right now; another machine controls them" });
    expect(rows.find((r) => r.id === "tunnels")?.mono).toBe(true);
  });

  it("the service row shows the three states the app reports and never spins without data", async () => {
    const d = await desk("demo&still&admin=pending");
    const rows = foundRows(d.machines.a!, { state: "pending", canInstall: true, note: "" });
    expect(rows.find((r) => r.id === "tunnels")).toMatchObject({ state: "ok", note: "zsh · Docker · uv · node" });
    expect(rows.at(-1)).toMatchObject({ id: "service", state: "wait", install: true });
    expect(serviceRow({ state: "off", canInstall: true, note: "" }, false)).toMatchObject({ state: "off", install: true });
    expect(serviceRow({ state: "ok", canInstall: false, note: "" }, false)).toMatchObject({ state: "ok", install: false });
    // Without a button, the row explains why (per-user install).
    expect(serviceRow({ state: "off", canInstall: false, note: "per-user install" }, false)).toMatchObject({ state: "off", note: "per-user install", install: false });
    // Without an app reading: the module decides and, if it is not active, `off`; never `wait`.
    expect(serviceRow(null, true).state).toBe("ok");
    expect(serviceRow(null, false).state).toBe("off");
  });

  it("without the machine there are no rows, and on Windows the monitors do have a channel", async () => {
    expect(foundRows(null)).toEqual([]);
    const d = await desk("demo&still");
    expect(foundRows(d.machines.b!).find((r) => r.id === "monitors")).toMatchObject({ state: "ok", note: "DDC channel" });
  });
});

describe("step 3", () => {
  it("counts what is missing and says it with the mockup texts", async () => {
    const d = await desk("demo&still&perms=missing");
    const cards = permCards(d.permissions.rows);
    expect(cards.map((c) => [c.title, c.granted])).toEqual([
      ["Accessibility", false],
      ["Input Monitoring", false],
      ["Local Network", true],
    ]);
    expect(missingOf(cards)).toBe(2);
    // What the agent cannot check (Local network on a real agent) neither blocks nor is taken as granted.
    const unchecked = permCards([{ id: "accessibility", state: "granted" }, { id: "local_network", state: "unknown" }]);
    expect(unchecked.map((c) => [c.granted, c.missing])).toEqual([[true, false], [false, false]]);
    expect(missingOf(unchecked)).toBe(0);
    // Local Network has its own texts and the "Ask again" action while it is not granted (T-344).
    const net = (state: "granted" | "missing" | "unknown") => permCards([{ id: "local_network", state }])[0]!;
    expect(net("unknown")).toMatchObject({ askAgain: true, missing: false, status: "Not checked yet · press “Ask again” and choose Allow when macOS asks" });
    expect(net("missing")).toMatchObject({ askAgain: true, missing: true, status: "Blocked by macOS · turn on Tandem (or tandemd) in System Settings → Privacy & Security → Local Network" });
    expect(net("granted")).toMatchObject({ askAgain: false, granted: true, status: "Granted · checked just now" });
    expect(permCards([{ id: "accessibility", state: "missing" }])[0]).toMatchObject({ askAgain: false, status: "Waiting… it is checked automatically" });
    // The demo shows each Local Network state (`net=denied|unknown`).
    const denied = await desk("demo&still&net=denied");
    expect(denied.permissions.rows.find((r) => r.id === "local_network")?.state).toBe("missing");
    const unknown = await desk("demo&still&perms=missing&net=unknown");
    expect(unknown.permissions.rows.find((r) => r.id === "local_network")?.state).toBe("unknown");
    expect(permFooter(2)).toBe("Missing 2");
    expect(permFooter(1)).toBe("Missing 1");
    expect(permFooter(0)).toBe("All granted");
  });
});

describe("step 4", () => {
  it("activity accumulates per machine and does not change what is already marked", () => {
    const a = heard(NO_ACTIVITY, "b");
    expect(a).toEqual({ a: false, b: true });
    expect(heard(a, "b")).toBe(a);
    expect(heard(a, "a")).toEqual({ a: true, b: true });
  });

  it("lists only what the machine announces: what is assumed does not count", async () => {
    const d = await desk();
    expect(deviceRows(d.machines.a!).map((x) => x.name)).toEqual(["Built-in keyboard", "Trackpad", "Keychron K3 · MX Master 3S"]);
    expect(deviceRows(d.machines.b!)).toEqual([]);
    expect(deviceRows(null)).toEqual([]);
    // With the input module active but no devices announced, there is no row.
    const { inputDevices: _omit, ...bare } = d.machines.a!;
    expect(deviceRows(bare)).toEqual([]);
  });

  it("the agent's hub is the only thing that counts as keyboard and mouse", () => {
    expect(devicesOfHub(true)).toEqual([{ name: "Keyboard and mouse", via: "through the hub" }]);
    expect(devicesOfHub(false)).toEqual([]);
    expect(devicesOfHub(null)).toEqual([]);
    expect(devicesOfHub(undefined)).toEqual([]);
  });
});

describe("step 5", () => {
  it("each edge has its drawing", () => {
    for (const side of ["left", "right", "up", "down"] as Side[]) {
      expect(SKETCH[side].here).toContain("left");
      expect(SKETCH[side].edge).toContain("width");
    }
  });

  it("dropSide picks the dominant side, and the arrows agree", () => {
    for (let dx = -40; dx <= 40; dx += 4) {
      for (let dy = -40; dy <= 40; dy += 4) {
        const side = dropSide(dx, dy);
        expect(side === "left" || side === "right" ? Math.abs(dx) >= Math.abs(dy) : Math.abs(dy) > Math.abs(dx)).toBe(true);
      }
    }
    expect([arrowSide("ArrowUp"), arrowSide("ArrowDown"), arrowSide("ArrowLeft"), arrowSide("ArrowRight"), arrowSide("Enter")]).toEqual(["up", "down", "left", "right", null]);
  });

  it("the verdict tells the truth of each phase", () => {
    const sample = { from: "a", to: "b", edge: "right", at: 1, of: 2, ms: 0.4 } as const;
    expect(crossVerdict(start(0), "right", "MacBook Pro", "PC", 20)).toMatchObject({ tone: "wait", text: "Waiting for the crossing at the right edge of MacBook Pro…" });
    expect(crossVerdict({ kind: "passed", sample }, "right", "M", "PC", 20)).toMatchObject({ tone: "ok", text: expect.stringContaining("Round trip measured: 0.4 ms") });
    expect(crossVerdict({ kind: "wrong-edge", sample: { ...sample, edge: "up" } }, "right", "M", "PC", 20).text).toContain("Crossed at the top, not the right");
    expect(crossVerdict({ kind: "timeout" }, "up", "M", "PC", 20).text).toContain("top edge");
    expect(crossVerdict({ kind: "unavailable", reason: "no probe" }, "up", "M", "PC", 20).tone).toBe("warn");
  });
});

describe("step 6", () => {
  it("the summary says what was checked and what was not", async () => {
    const d = await desk();
    const ok = summaryRows(d, { crossed: true });
    expect(ok.map((r) => [r.id, r.ok])).toEqual([
      ["link", true],
      ["input", true],
      ["monitors", true],
      ["tunnels", true],
    ]);
    expect(ok[0]?.note).toBe("USB-C · 0.3 ms");
    expect(ok[2]?.note).toContain("owner PC-Desktop");
    const no = summaryRows(d, { crossed: false });
    expect(no[1]).toMatchObject({ ok: false, note: "crossing not tested" });
  });

  it("remote compute requires the module on and a ready environment", async () => {
    const d = await desk();
    type M = NonNullable<typeof d.machines.a>;
    const setTunnels = (m: M, state: "off" | "active" | "unavailable") => ({ ...m, modules: m.modules.map((x) => (x.id === "tunnels" ? { ...x, state } : x)) });
    const tunnels = (machines: typeof d.machines) => summaryRows({ ...d, machines }, { crossed: true }).find((r) => r.id === "tunnels");
    const a = d.machines.a!;
    const b = d.machines.b!;
    expect(a.environments.some((e) => e.ready)).toBe(true);
    expect(tunnels({ a, b })).toMatchObject({ ok: true, note: "MCP ready for Claude Code on both" });
    // Tunnels off on one machine, even if environments were detected.
    expect(tunnels({ a, b: setTunnels(b, "off") })?.ok).toBe(false);
    expect(tunnels({ a, b: setTunnels(b, "unavailable") })?.ok).toBe(false);
    // Environments with none ready.
    const noReady = { ...b, environments: b.environments.map((e) => ({ ...e, ready: false })) };
    expect(tunnels({ a, b: noReady })?.ok).toBe(false);
    expect(computeWorks(null)).toBe(false);
  });
});
