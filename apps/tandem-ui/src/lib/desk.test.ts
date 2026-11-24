import { describe, expect, it } from "vitest";

import { demoDesk, demoOptions } from "./store/demo";
import { cursorPoint, DEMO_LOOP_MS, demoPath, flowSeconds, RateMeter, sampleAt, sceneOf } from "./desk";

const names = { a: "MacBook Pro", b: "PC-Desktop" };
const colors = { a: "#7aa7e6", b: "#a99be8" };
const desk = () => demoDesk(demoOptions("demo&still"));
const scene = () => sceneOf(desk().layout, names, colors);

describe("scaled desktops (V3-Desk)", () => {
  it("the demo lands where the mockup draws the MacBook and the 34\"", () => {
    const s = scene();
    const mac = s.desks.find((d) => d.machine === "a");
    const pc = s.desks.find((d) => d.machine === "b");
    // Mockup measurements, in % of the canvas (half a point of tolerance).
    const near = (v: number | undefined, want: number) => expect(Math.abs((v ?? 99) - want)).toBeLessThan(0.5);
    near(mac?.left, 7.5);
    near(mac?.top, 36);
    near(mac?.width, 22.9);
    near(mac?.height, 35.8);
    near(pc?.left, 32.6);
    near(pc?.top, 10.9);
    near(pc?.width, 60);
    near(pc?.height, 60.9);
    expect(s.aspect).toBeCloseTo(133 / 55, 2);
  });

  it("names and colors come from each machine", () => {
    const s = scene();
    expect(s.desks.map((d) => d.label)).toEqual(["MacBook Pro", '34" · PC-Desktop']);
    const other = sceneOf(desk().layout, { a: "Studio", b: "Workshop" }, { a: "#5fbfb0", b: "#e6a07a" });
    expect(other.desks.map((d) => d.label)).toEqual(["Studio", '34" · Workshop']);
    expect(other.desks[0]?.wall).not.toBe(s.desks[0]?.wall);
  });

  it("the MacBook has a base and the monitor a support and a stand", () => {
    expect(scene().feet).toHaveLength(3);
    expect(scene().feet.filter((f) => f.radius === 6)).toHaveLength(1);
  });

  it("the crossing edge falls on the side of B where A is and measures what they share", () => {
    const s = scene();
    const pc = s.desks.find((d) => d.machine === "b");
    expect(s.edge?.vertical).toBe(true);
    expect(s.edge?.left).toBeCloseTo(pc?.left ?? 0, 5);
    // With A at the very bottom (align end) the shared stretch is the MacBook's height.
    expect(s.edge?.length).toBeGreaterThan(30);
    expect(s.edge?.length).toBeLessThan(40);
  });

  it("without screens there is nothing to draw", () => {
    const s = sceneOf({ ...desk().layout, screens: [] }, names, colors);
    expect(s.desks).toEqual([]);
    expect(s.edge).toBeNull();
  });
});

describe("cursor over the desktop", () => {
  const layout = () => desk().layout;

  it("the center of the native screen falls on the center of its box", () => {
    const s = scene();
    const mac = s.desks.find((d) => d.machine === "a");
    const p = cursorPoint(s, layout(), { machine: "a", screen: "mbp", x: 756, y: 491 });
    expect(p?.left).toBeCloseTo((mac?.left ?? 0) + (mac?.width ?? 0) / 2, 5);
    expect(p?.top).toBeCloseTo((mac?.top ?? 0) + (mac?.height ?? 0) / 2, 5);
  });

  it("without a screen id it is looked up by the point, and outside the screen it stays on the edge", () => {
    const s = scene();
    const pc = s.desks.find((d) => d.machine === "b");
    const p = cursorPoint(s, layout(), { machine: "b", screen: "", x: 99999, y: -5 });
    expect(p?.left).toBeCloseTo((pc?.left ?? 0) + (pc?.width ?? 0), 5);
    expect(p?.top).toBeCloseTo(pc?.top ?? 0, 5);
  });

  it("a machine without known screens is not drawn", () => {
    const l = { ...layout(), screens: layout().screens.map((x) => ({ ...x, native: undefined })) };
    expect(cursorPoint(scene(), l, { machine: "a", screen: "", x: 1, y: 1 })).toBeNull();
  });

  it("round trip: from the point to the sample and from the sample to the point", () => {
    const s = scene();
    for (const d of s.desks) {
      for (const fx of [0.05, 0.3, 0.5, 0.95]) {
        for (const fy of [0.05, 0.5, 0.95]) {
          const pt = { left: d.left + d.width * fx, top: d.top + d.height * fy };
          const sample = sampleAt(s, layout(), pt);
          expect(sample?.machine).toBe(d.machine);
          const back = sample ? cursorPoint(s, layout(), sample) : null;
          expect(back?.left).toBeCloseTo(pt.left, 6);
          expect(back?.top).toBeCloseTo(pt.top, 6);
        }
      }
    }
  });

  it("between the two screens there is no sample", () => {
    expect(sampleAt(scene(), layout(), { left: 31.5, top: 50 })).toBeNull();
  });
});

describe("link traffic", () => {
  it("counts events per second in the window", () => {
    const m = new RateMeter();
    for (let t = 0; t < 1000; t += 100) m.push(t);
    expect(m.rate(999)).toBe(10);
    expect(m.rate(1450)).toBe(5);
    expect(m.rate(5000)).toBe(0);
  });

  it("the pulse goes faster with more traffic", () => {
    expect(flowSeconds(0)).toBeGreaterThan(flowSeconds(5));
    expect(flowSeconds(5)).toBeGreaterThan(flowSeconds(30));
    expect(flowSeconds(30)).toBe(1);
    expect(flowSeconds(Number.NaN)).toBe(flowSeconds(0));
  });
});

describe("demo path (the `cur` keyframes)", () => {
  it("starts and ends at the same point and passes through those of the mockup", () => {
    expect(demoPath(0)).toEqual({ left: 16, top: 56 });
    expect(demoPath(1)).toEqual({ left: 16, top: 56 });
    expect(demoPath(0.38)).toEqual({ left: 40, top: 48 });
    expect(demoPath(0.62)).toEqual({ left: 63, top: 30 });
    expect(DEMO_LOOP_MS).toBe(7000);
  });

  it("always stays inside the canvas", () => {
    for (let i = 0; i <= 200; i++) {
      const p = demoPath(i / 200);
      expect(p.left).toBeGreaterThanOrEqual(0);
      expect(p.left).toBeLessThanOrEqual(100);
      expect(p.top).toBeGreaterThanOrEqual(0);
      expect(p.top).toBeLessThanOrEqual(100);
    }
  });
});

describe("the real two-monitor desk (`desk=g5`, T-356)", () => {
  const g5 = () => demoDesk(demoOptions("demo&desk=g5&still"));

  it("draws the 34\" ultrawide and the 32\" G5 at their physical proportions, labelled by size", () => {
    const d = g5();
    const s = sceneOf(d.layout, { a: "Mac", b: "DESKTOP-7Q4K2M9" }, colors);
    const [wide, std] = s.desks;
    expect(s.desks.map((k) => k.label)).toEqual(['34" · Mac', '32" · Mac']);
    // In canvas units: width over height, corrected by the canvas aspect (the mockup draws a
    // monitor 0.5 % taller than its measurement).
    const ratio = (k: { width: number; height: number }) => (k.width * s.aspect) / k.height;
    expect(ratio(wide!)).toBeCloseTo(797 / 334 / 1.005, 2);
    expect(ratio(std!)).toBeCloseTo(700 / 400 / 1.005, 2);
    // The 34" is wider and lower than the 32".
    expect(wide!.width).toBeGreaterThan(std!.width);
    expect(wide!.height).toBeLessThan(std!.height);
  });

  it("each model stands on its own base, both on the same table", () => {
    const s = sceneOf(g5().layout, { a: "Mac", b: "PC" }, colors);
    const bases = s.feet.filter((f) => f.shape);
    expect(bases.map((f) => f.shape)).toEqual(["v", "trapezoid"]);
    expect(bases[0]!.top).toBeCloseTo(bases[1]!.top, 6);
    // Each neck reaches from its panel to the table.
    for (const k of s.desks) {
      const neck = s.feet.find((f) => !f.shape && f.left > k.left && f.left < k.left + k.width);
      expect(neck!.top).toBeCloseTo(k.top + k.height, 6);
      expect(neck!.top + neck!.height).toBeCloseTo(bases[0]!.top, 6);
    }
    expect(Math.max(...s.feet.map((f) => f.top + f.height))).toBeLessThanOrEqual(100);
  });

  it("a monitor that is not a known model keeps the mockup's stand", () => {
    const s = scene();
    expect(s.feet.some((f) => f.shape)).toBe(false);
  });

  it("the scenes name the monitors by size, so long machine names still fit", () => {
    const d = g5();
    expect(d.scenes.map((x) => x.desc)).toEqual(['34" → DESKTOP-7Q4K2M9, 32" → Mac', '34" → DESKTOP-7Q4K2M9, 32" → DESKTOP-7Q4K2M9', '34" → Mac, 32" → Mac']);
  });
});

