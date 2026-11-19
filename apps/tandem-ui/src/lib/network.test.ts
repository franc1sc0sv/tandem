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

describe("textos", () => {
  it("la latencia en ms con un decimal; sin dato, raya", () => {
    expect([0.3, 4, 3.8, 14.2, 120, 0.04].map(formatMs)).toEqual(["0.3 ms", "4 ms", "3.8 ms", "14 ms", "120 ms", "<0.1 ms"]);
    expect(formatMs(null)).toBe("—");
  });

  it("el ancho de banda pasa de Mbps a Gbps en 1", () => {
    expect([9.4, 1, 0.41, 0.005].map(formatRate)).toEqual(["9.4 Gbps", "1 Gbps", "410 Mbps", "5 Mbps"]);
    expect(formatRate(null)).toBe("—");
    expect(measuredText("wifi", 0.41)).toBe("Wi-Fi: 410 Mbps");
  });

  it("segundos con un decimal, y bytes por segundo a Gbps", () => {
    expect(formatSecs(600)).toBe("0.6 s");
    expect(formatSecs(1250)).toBe("1.3 s");
    expect(gbpsOf(1_175_000_000)).toBe(9.4);
    expect(gbpsOf(51_250_000)).toBe(0.41);
    expect(gbpsOf(null)).toBeNull();
  });
});

describe("del agente a la pantalla", () => {
  const wire: NetworkWire = {
    paths: [
      { link: "wifi", state: "backup", rtt_us: 3800, input_p50_us: null, input_p99_us: null, input_target_us: 20_000, bytes_per_sec: 51_250_000, history: [{ rtt_us: 3800, input_p99_us: null }] },
      { link: "usb4", state: "primary", rtt_us: 300, input_p50_us: 400, input_p99_us: 1900, input_target_us: 4000, bytes_per_sec: 1_175_000_000, history: [{ rtt_us: 300, input_p99_us: 1900 }] },
    ],
    switches: [{ at_ms: 5, from: "usb4", to: "wifi", took_ms: 600 }],
    measuring: "wifi",
  };

  it("pone los tres caminos en su orden, en ms y Gbps", () => {
    const n = networkFromWire(wire, 99);
    expect(n.paths.map((p) => p.kind)).toEqual(["usb4", "ethernet", "wifi"]);
    expect(path(n, "usb4")).toMatchObject({ state: "primary", rttMs: 0.3, inputP50Ms: 0.4, inputP99Ms: 1.9, targetMs: 4, gbps: 9.4, history: [{ rttMs: 0.3, inputP99Ms: 1.9 }] });
    expect(path(n, "wifi")).toMatchObject({ state: "backup", gbps: 0.41, targetMs: 20 });
    // Un camino que el agente no mandó queda no cable.
    expect(path(n, "ethernet")).toMatchObject({ state: "unplugged", rttMs: null, history: [] });
    expect(n.switches).toEqual([{ at: 5, from: "usb4", to: "wifi", tookMs: 600 }]);
    expect(n.measuring).toBe("wifi");
    expect(n.now).toBe(99);
  });
});

describe("indicadores", () => {
  it("el anillo de latencia es el p99 contra el objetivo del principal", () => {
    expect(latencyKpi(demo())).toMatchObject({ p50: 0.4, p99: 1.9, tone: "ok" });
    expect(latencyKpi(demo()).load).toBeCloseTo(0.475, 3);
    const hot = (p99: number): NetworkView => ({ ...demo(), paths: demo().paths.map((p) => (p.state === "primary" ? { ...p, inputP99Ms: p99 } : p)) });
    expect(latencyKpi(hot(3.5)).tone).toBe("warn");
    expect(latencyKpi(hot(4)).tone).toBe("bad");
    expect(latencyKpi(hot(40)).load).toBe(1);
    // Sin principal o sin entrada: nada que decir.
    expect(latencyKpi(EMPTY_NETWORK)).toEqual({ p50: null, p99: null, load: 0, tone: "ok" });
  });

  it("el anillo de ancho de banda sale del principal y se llena con 10 Gbps", () => {
    expect(bandwidthKpi(demo())).toEqual({ gbps: 9.4, via: "usb4", fill: 0.94 });
    expect(bandwidthKpi(EMPTY_NETWORK)).toEqual({ gbps: null, via: null, fill: 0 });
    const unmeasured: NetworkView = { ...demo(), paths: demo().paths.map((p) => ({ ...p, gbps: null })) };
    expect(bandwidthKpi(unmeasured)).toEqual({ gbps: null, via: "usb4", fill: 0 });
  });

  it("las barras de las tarjetas no se salen de 0–100", () => {
    expect([0.3, 3.8, 50, null].map(latencyBar)).toEqual([3, 38, 100, 0]);
    expect(bandwidthBar("usb4", 9.4)).toBe(94);
    expect(bandwidthBar("wifi", 0.41)).toBe(12);
    expect(bandwidthBar("ethernet", 99)).toBe(100);
    expect(bandwidthBar("wifi", null)).toBe(0);
  });

  it("describe cada camino para quien no lo ve", () => {
    expect(demo().paths.map(describePath)).toEqual(["USB-C active at 0.3 ms", "Ethernet no cable", "Wi-Fi as backup at 3.8 ms"]);
    expect(describePath({ ...path(demo(), "usb4"), state: "down" })).toBe("USB-C down");
  });
});

describe("la gráfica de 60 s", () => {
  it("en cada segundo toma la latencia del camino que llevaba la entrada", () => {
    const s = latencySeries(demo());
    expect(s).toHaveLength(HISTORY_SECS);
    // El cable lleva la entrada; en el corte (23–30) la lleva el Wi-Fi, con el pico.
    expect(s[0]).toBe(1.2);
    expect(s[27]).toBe(5.6);
    expect(s[31]).toBe(1.3);
    expect(s.every((v) => v !== null)).toBe(true);
  });

  it("sin entrada en ningún camino usa el ida y vuelta de los pings; sin nada, todo vacío", () => {
    const noInput: NetworkView = { ...demo(), paths: demo().paths.map((p) => ({ ...p, history: p.history.map((h) => ({ ...h, inputP99Ms: null })) })) };
    const s = latencySeries(noInput);
    expect(s.every((v) => v !== null)).toBe(true);
    expect(s[0]).toBe(path(noInput, "usb4").history[0]!.rttMs);
    expect(latencySeries(EMPTY_NETWORK).every((v) => v === null)).toBe(true);
  });

  it("un historial corto queda to the right of: lo viejo que falta es hueco", () => {
    const short: NetworkView = { ...demo(), paths: demo().paths.map((p) => ({ ...p, history: p.history.slice(-3) })) };
    const s = latencySeries(short);
    expect(s.slice(0, 57).every((v) => v === null)).toBe(true);
    expect(s.slice(57).every((v) => v !== null)).toBe(true);
  });

  it("la geometría es la del mockup: objetivo de 4 ms en y = 34 y el pico en y = 18", () => {
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

  it("un hueco corta la línea y todo punto cae dentro del lienzo", () => {
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

  it("sin muestras no hay línea ni pico; con todo bajo el objetivo, tampoco pico", () => {
    const none = chartGeometry(Array.from({ length: 60 }, () => null), 4);
    expect(none).toMatchObject({ line: "", area: "", peak: null, hasData: false });
    expect(none.targetY).toBeCloseTo(34, 0);
    expect(chartGeometry([1, 1.2, 1], 4).peak).toBeNull();
    expect(chartGeometry([1, 1.2, 1], null)).toMatchObject({ targetMs: null, targetY: null, peak: null });
  });

  it("la escala crece si el pico lo pide, sin salirse del lienzo", () => {
    for (const top of [1, 4.2, 7.4, 20, 500]) {
      const g = chartGeometry([1, top, 1], 4);
      expect(g.peak?.y ?? CHART.height).toBeGreaterThanOrEqual(0);
      expect(g.peak?.y ?? 0).toBeLessThanOrEqual(CHART.height);
    }
  });

  it("el objetivo es el del camino principal (el de respaldo si no hay)", () => {
    expect(chartTarget(demo())).toBe(4);
    const wifiOnly: NetworkView = { ...demo(), paths: demo().paths.map((p) => (p.kind === "usb4" ? { ...p, state: "unplugged" } : p)) };
    expect(chartTarget(wifiOnly)).toBe(20);
    expect(chartTarget(EMPTY_NETWORK)).toBeNull();
  });
});

describe("la nota del último cambio", () => {
  it("dice el corte con lo que tardó, como el mockup", () => {
    expect(pathNote(demo(), true)).toEqual({ tone: "warn", text: "Spike when the cable was unplugged · switched to Wi-Fi in 0.6 s" });
    expect(pathNote(demo(), false)?.text).toBe("The cable was unplugged · switched to Wi-Fi in 0.6 s");
  });

  it("si cayó el Wi-Fi, o no quedó camino", () => {
    const base = demo();
    const cut = (from: "usb4" | "wifi", to: "usb4" | "wifi" | null): NetworkView => ({ ...base, switches: [{ at: DEMO_NOW - 5000, from, to, tookMs: 1200 }] });
    expect(pathNote(cut("wifi", "usb4"), true)?.text).toBe("Spike when Wi-Fi dropped · switched to USB-C in 1.2 s");
    expect(pathNote(cut("wifi", "usb4"), false)?.text).toBe("Wi-Fi dropped · switched to USB-C in 1.2 s");
    expect(pathNote(cut("usb4", null), false)?.text).toBe("The cable was unplugged · no path for 1.2 s");
  });

  it("sin corte, el último cambio; fuera de los 60 s o sin cambios, nada", () => {
    const base = demo();
    const up = (from: "wifi" | null): NetworkView => ({ ...base, switches: [{ at: DEMO_NOW - 1000, from, to: "usb4", tookMs: null }] });
    expect(pathNote(up("wifi"), false)).toEqual({ tone: "info", text: "Switched from Wi-Fi to USB-C" });
    expect(pathNote(up(null), false)).toEqual({ tone: "info", text: "Connected over USB-C" });
    // El corte del demo fue hace 36 s: sale de la gráfica 24 s después, y la vuelta del cable 32 s después.
    expect(pathNote({ ...base, now: DEMO_NOW + 24_000 }, true)?.tone).toBe("warn");
    expect(pathNote({ ...base, now: DEMO_NOW + 25_000 }, true)).toEqual({ tone: "info", text: "Switched from Wi-Fi to USB-C" });
    expect(pathNote({ ...base, now: DEMO_NOW + 40_000 }, true)).toBeNull();
    expect(pathNote(EMPTY_NETWORK, false)).toBeNull();
  });
});

describe("camino sin respuesta (T-147)", () => {
  const kinds = ["usb4", "ethernet", "wifi"] as const;
  const mk = (kind: PathView["kind"], state: PathView["state"]): PathView => ({ ...path(demo(), "usb4"), kind, state });

  it("el Wi-Fi no tiene cable: dice «no signal»; USB-C y Ethernet, «no cable»", () => {
    expect(unpluggedText("wifi")).toBe("no signal");
    expect(unpluggedText("usb4")).toBe("no cable");
    expect(unpluggedText("ethernet")).toBe("no cable");
  });

  it("los tres caminos × unplugged y down", () => {
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
