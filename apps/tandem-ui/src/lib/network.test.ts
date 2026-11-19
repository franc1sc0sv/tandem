import { describe, expect, it } from "vitest";

import {
  CHART,
  HISTORY_SECS,
  bandwidthBar,
  bandwidthKpi,
  chartGeometry,
  chartTarget,
  describePath,
  formatMs,
  formatRate,
  formatSecs,
  gbpsOf,
  latencyBar,
  latencyKpi,
  latencySeries,
  measuredText,
  networkFromWire,
  pathNote,
  stateLabel,
  unpluggedText,
  type NetworkWire,
} from "./network";
import { LINK_NAME } from "./machines";
import { DEMO_NOW, demoNetwork } from "./store/demo";
import { EMPTY_NETWORK, type NetworkView, type PathView } from "./store/types";

const demo = () => demoNetwork(true);
const path = (n: NetworkView, kind: PathView["kind"]) => n.paths.find((p) => p.kind === kind)!;

describe("texts", () => {
  it("latency in ms with one decimal; without data, a dash", () => {
    expect([0.3, 4, 3.8, 14.2, 120, 0.04].map(formatMs)).toEqual(["0.3 ms", "4 ms", "3.8 ms", "14 ms", "120 ms", "<0.1 ms"]);
    expect(formatMs(null)).toBe("—");
  });

  it("bandwidth goes from Mbps to Gbps at 1", () => {
    expect([9.4, 1, 0.41, 0.005].map(formatRate)).toEqual(["9.4 Gbps", "1 Gbps", "410 Mbps", "5 Mbps"]);
    expect(formatRate(null)).toBe("—");
    expect(measuredText("wifi", 0.41)).toBe("Wi-Fi: 410 Mbps");
  });

  it("seconds with one decimal, and bytes per second to Gbps", () => {
    expect(formatSecs(600)).toBe("0.6 s");
    expect(formatSecs(1250)).toBe("1.3 s");
    expect(gbpsOf(1_175_000_000)).toBe(9.4);
    expect(gbpsOf(51_250_000)).toBe(0.41);
    expect(gbpsOf(null)).toBeNull();
  });
});

describe("from the agent to the screen", () => {
  const wire: NetworkWire = {
    paths: [
      { link: "wifi", state: "backup", rtt_us: 3800, input_p50_us: null, input_p99_us: null, input_target_us: 20_000, bytes_per_sec: 51_250_000, history: [{ rtt_us: 3800, input_p99_us: null }] },
      { link: "usb4", state: "primary", rtt_us: 300, input_p50_us: 400, input_p99_us: 1900, input_target_us: 4000, bytes_per_sec: 1_175_000_000, history: [{ rtt_us: 300, input_p99_us: 1900 }] },
    ],
    switches: [{ at_ms: 5, from: "usb4", to: "wifi", took_ms: 600 }],
    measuring: "wifi",
  };

  it("puts the three paths in their order, in ms and Gbps", () => {
    const n = networkFromWire(wire, 99);
    expect(n.paths.map((p) => p.kind)).toEqual(["usb4", "ethernet", "wifi"]);
    expect(path(n, "usb4")).toMatchObject({ state: "primary", rttMs: 0.3, inputP50Ms: 0.4, inputP99Ms: 1.9, targetMs: 4, gbps: 9.4, history: [{ rttMs: 0.3, inputP99Ms: 1.9 }] });
    expect(path(n, "wifi")).toMatchObject({ state: "backup", gbps: 0.41, targetMs: 20 });
    // A path the agent did not send ends up as no cable.
    expect(path(n, "ethernet")).toMatchObject({ state: "unplugged", rttMs: null, history: [] });
    expect(n.switches).toEqual([{ at: 5, from: "usb4", to: "wifi", tookMs: 600 }]);
    expect(n.measuring).toBe("wifi");
    expect(n.now).toBe(99);
  });
});

describe("indicadores", () => {
  it("the latency ring is the p99 against the primary's target", () => {
    expect(latencyKpi(demo())).toMatchObject({ p50: 0.4, p99: 1.9, tone: "ok" });
    expect(latencyKpi(demo()).load).toBeCloseTo(0.475, 3);
    const hot = (p99: number): NetworkView => ({ ...demo(), paths: demo().paths.map((p) => (p.state === "primary" ? { ...p, inputP99Ms: p99 } : p)) });
    expect(latencyKpi(hot(3.5)).tone).toBe("warn");
    expect(latencyKpi(hot(4)).tone).toBe("bad");
    expect(latencyKpi(hot(40)).load).toBe(1);
    // Without a primary or without input: nothing to say.
    expect(latencyKpi(EMPTY_NETWORK)).toEqual({ p50: null, p99: null, load: 0, tone: "ok" });
  });

  it("the bandwidth ring comes from the primary and fills at 10 Gbps", () => {
    expect(bandwidthKpi(demo())).toEqual({ gbps: 9.4, via: "usb4", fill: 0.94 });
    expect(bandwidthKpi(EMPTY_NETWORK)).toEqual({ gbps: null, via: null, fill: 0 });
    const unmeasured: NetworkView = { ...demo(), paths: demo().paths.map((p) => ({ ...p, gbps: null })) };
    expect(bandwidthKpi(unmeasured)).toEqual({ gbps: null, via: "usb4", fill: 0 });
  });

  it("the card bars stay within 0–100", () => {
    expect([0.3, 3.8, 50, null].map(latencyBar)).toEqual([3, 38, 100, 0]);
    expect(bandwidthBar("usb4", 9.4)).toBe(94);
    expect(bandwidthBar("wifi", 0.41)).toBe(12);
    expect(bandwidthBar("ethernet", 99)).toBe(100);
    expect(bandwidthBar("wifi", null)).toBe(0);
  });

  it("describes each path for someone who cannot see it", () => {
    expect(demo().paths.map(describePath)).toEqual(["USB-C active at 0.3 ms", "Ethernet no cable", "Wi-Fi as backup at 3.8 ms"]);
    expect(describePath({ ...path(demo(), "usb4"), state: "down" })).toBe("USB-C down");
  });
});

describe("the 60 s chart", () => {
  it("in each second takes the latency of the path that carried the input", () => {
    const s = latencySeries(demo());
    expect(s).toHaveLength(HISTORY_SECS);
    // The cable carries the input; at the cut (23–30) Wi-Fi carries it, with the peak.
    expect(s[0]).toBe(1.2);
    expect(s[27]).toBe(5.6);
    expect(s[31]).toBe(1.3);
    expect(s.every((v) => v !== null)).toBe(true);
  });

  it("without input on any path it uses the ping round trip; without anything, all empty", () => {
    const noInput: NetworkView = { ...demo(), paths: demo().paths.map((p) => ({ ...p, history: p.history.map((h) => ({ ...h, inputP99Ms: null })) })) };
    const s = latencySeries(noInput);
    expect(s.every((v) => v !== null)).toBe(true);
    expect(s[0]).toBe(path(noInput, "usb4").history[0]!.rttMs);
    expect(latencySeries(EMPTY_NETWORK).every((v) => v === null)).toBe(true);
  });

  it("a short history is right-aligned: the missing old part is a gap", () => {
    const short: NetworkView = { ...demo(), paths: demo().paths.map((p) => ({ ...p, history: p.history.slice(-3) })) };
    const s = latencySeries(short);
    expect(s.slice(0, 57).every((v) => v === null)).toBe(true);
    expect(s.slice(57).every((v) => v !== null)).toBe(true);
  });

  it("the geometry is the mockup's: 4 ms target at y = 34 and the peak at y = 18", () => {
    const n = demo();
    const g = chartGeometry(latencySeries(n), chartTarget(n));
    expect(g.targetMs).toBe(4);
    expect(g.targetY).toBeCloseTo(34, 0);
    expect(g.peak).toMatchObject({ ms: 5.6 });
    expect(g.peak?.y).toBeCloseTo(18, 0);
    expect(g.peak?.x).toBeCloseTo((27 * CHART.width) / (HISTORY_SECS - 1), 1);
    expect(g.line.startsWith("M0 ")).toBe(true);
    expect(g.line.endsWith(`${CHART.width} ${g.line.split(" ").at(-1)}`)).toBe(true);
    expect(g.area.endsWith("Z")).toBe(true);
    expect(g.hasData).toBe(true);
  });

  it("a gap cuts the line and every point falls inside the canvas", () => {
    const series = [1, 2, null, null, 3, 8, 100];
    const g = chartGeometry(series, 4);
    expect(g.line.match(/M/g)).toHaveLength(2);
    const nums = [...g.line.matchAll(/[ML]([\d.]+) ([\d.]+)/g)].map((m) => [Number(m[1]), Number(m[2])] as const);
    for (const [x, y] of nums) {
      expect(x).toBeGreaterThanOrEqual(0);
      expect(x).toBeLessThanOrEqual(CHART.width);
      expect(y).toBeGreaterThanOrEqual(0);
      expect(y).toBeLessThanOrEqual(CHART.height);
    }
    expect(g.peak?.ms).toBe(100);
  });

  it("without samples there is no line or peak; with everything under the target, no peak either", () => {
    const none = chartGeometry(Array.from({ length: 60 }, () => null), 4);
    expect(none).toMatchObject({ line: "", area: "", peak: null, hasData: false });
    expect(none.targetY).toBeCloseTo(34, 0);
    expect(chartGeometry([1, 1.2, 1], 4).peak).toBeNull();
    expect(chartGeometry([1, 1.2, 1], null)).toMatchObject({ targetMs: null, targetY: null, peak: null });
  });

  it("the scale grows if the peak asks for it, without leaving the canvas", () => {
    for (const top of [1, 4.2, 7.4, 20, 500]) {
      const g = chartGeometry([1, top, 1], 4);
      expect(g.peak?.y ?? CHART.height).toBeGreaterThanOrEqual(0);
      expect(g.peak?.y ?? 0).toBeLessThanOrEqual(CHART.height);
    }
  });

  it("the target is that of the primary path (the backup's if there is none)", () => {
    expect(chartTarget(demo())).toBe(4);
    const wifiOnly: NetworkView = { ...demo(), paths: demo().paths.map((p) => (p.kind === "usb4" ? { ...p, state: "unplugged" } : p)) };
    expect(chartTarget(wifiOnly)).toBe(20);
    expect(chartTarget(EMPTY_NETWORK)).toBeNull();
  });
});

describe("the note of the last change", () => {
  it("states the cut with how long it took, like the mockup", () => {
    expect(pathNote(demo(), true)).toEqual({ tone: "warn", text: "Spike when the cable was unplugged · switched to Wi-Fi in 0.6 s" });
    expect(pathNote(demo(), false)?.text).toBe("The cable was unplugged · switched to Wi-Fi in 0.6 s");
  });

  it("if Wi-Fi went down, or no path was left", () => {
    const base = demo();
    const cut = (from: "usb4" | "wifi", to: "usb4" | "wifi" | null): NetworkView => ({ ...base, switches: [{ at: DEMO_NOW - 5000, from, to, tookMs: 1200 }] });
    expect(pathNote(cut("wifi", "usb4"), true)?.text).toBe("Spike when Wi-Fi dropped · switched to USB-C in 1.2 s");
    expect(pathNote(cut("wifi", "usb4"), false)?.text).toBe("Wi-Fi dropped · switched to USB-C in 1.2 s");
    expect(pathNote(cut("usb4", null), false)?.text).toBe("The cable was unplugged · no path for 1.2 s");
  });

  it("without a cut, the last change; outside the 60 s or without changes, nothing", () => {
    const base = demo();
    const up = (from: "wifi" | null): NetworkView => ({ ...base, switches: [{ at: DEMO_NOW - 1000, from, to: "usb4", tookMs: null }] });
    expect(pathNote(up("wifi"), false)).toEqual({ tone: "info", text: "Switched from Wi-Fi to USB-C" });
    expect(pathNote(up(null), false)).toEqual({ tone: "info", text: "Connected over USB-C" });
    // The demo cut was 36 s ago: it leaves the chart 24 s later, and the cable's return 32 s later.
    expect(pathNote({ ...base, now: DEMO_NOW + 24_000 }, true)?.tone).toBe("warn");
    expect(pathNote({ ...base, now: DEMO_NOW + 25_000 }, true)).toEqual({ tone: "info", text: "Switched from Wi-Fi to USB-C" });
    expect(pathNote({ ...base, now: DEMO_NOW + 40_000 }, true)).toBeNull();
    expect(pathNote(EMPTY_NETWORK, false)).toBeNull();
  });
});

describe("path with no reply (T-147)", () => {
  const kinds = ["usb4", "ethernet", "wifi"] as const;
  const mk = (kind: PathView["kind"], state: PathView["state"]): PathView => ({ ...path(demo(), "usb4"), kind, state });

  it("Wi-Fi has no cable: it says \"no signal\"; USB-C and Ethernet, \"no cable\"", () => {
    expect(unpluggedText("wifi")).toBe("no signal");
    expect(unpluggedText("usb4")).toBe("no cable");
    expect(unpluggedText("ethernet")).toBe("no cable");
  });

  it("the three paths × unplugged and down", () => {
    for (const k of kinds) {
      const unplugged = k === "wifi" ? "No signal" : "No cable";
      expect(stateLabel(mk(k, "unplugged"))).toBe(unplugged);
      expect(stateLabel(mk(k, "down"))).toBe("Down");
      expect(describePath(mk(k, "unplugged"))).toBe(`${LINK_NAME[k]} ${unpluggedText(k)}`);
      expect(describePath(mk(k, "down"))).toBe(`${LINK_NAME[k]} down`);
      expect(describePath(mk(k, "unplugged"))).not.toMatch(/Wi-Fi.*cable/);
    }
  });
});
