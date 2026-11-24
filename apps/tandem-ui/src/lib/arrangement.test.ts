import { describe, expect, it } from "vitest";

import { ALIGNS, DEFAULT_ARRANGEMENT, OPPOSITE, SIDES, alignLabels, draw, exitSide, fillOf, initialSelection, panelFor, placeOther, placePhysical, ratioLabel, wallOf, type Box, type LayoutScreen, type LayoutView } from "./arrangement";
import { demoDesk, demoOptions } from "./store/demo";

const screen = (id: string, machine: "a" | "b" | null, w: number, h: number, usable = true, x = 0, y = 0): LayoutScreen => ({
  id,
  machine,
  x,
  y,
  name: id,
  short: id,
  ask: id,
  w,
  h,
  size: "—",
  res: "—",
  ratio: "—",
  scale: "—",
  usable,
  line: "",
});

const MOCK: LayoutView = demoDesk(demoOptions("demo&still")).layout;

/** What the mockup computes (`V3-Arrangement`, `renderVals`) for the Mac, the 34" and the 32". */
function mockupBoxes(side: string, al: string) {
  const W = 76,
    H = 26,
    G = 0.3;
  const S = { w: 31.4, h: 13.1 },
    M = { w: 12, h: 7.8 },
    D = { w: 27.9, h: 15.7 };
  const k = { start: 0, center: 0.5, end: 1 }[al] ?? 0;
  let mx: number, my: number;
  if (side === "left" || side === "right") {
    mx = side === "left" ? -M.w - G : S.w + G;
    my = (S.h - M.h) * k;
  } else {
    my = side === "up" ? -M.h - G : S.h + G;
    mx = (S.w - M.w) * k;
  }
  const dx = Math.max(S.w, side === "right" ? mx + M.w : 0) + 1.4,
    dy = S.h - D.h;
  const minx = Math.min(0, mx),
    maxx = dx + D.w,
    miny = Math.min(0, my, dy),
    maxy = Math.max(S.h, my + M.h);
  const ox = (W - (maxx - minx)) / 2 - minx,
    oy = (H - (maxy - miny)) / 2 - miny;
  const pct = (x: number, y: number, w: number, h: number) => ({ left: ((x + ox) / W) * 100, top: ((y + oy) / H) * 100, width: (w / W) * 100, height: (h / H) * 100 });
  return { mbp: pct(mx, my, M.w, M.h), g34: pct(0, 0, S.w, S.h), g32: pct(dx, dy, D.w, D.h) };
}

const overlap = (a: Box, b: Box) => a.left < b.left + b.width - 1e-9 && b.left < a.left + a.width - 1e-9 && a.top < b.top + b.height - 1e-9 && b.top < a.top + a.height - 1e-9;

/** Seeded generator: the same property tests on every run. */
function rng(seed: number) {
  let s = seed;
  return () => {
    s = (s * 1664525 + 1013904223) % 4294967296;
    return s / 4294967296;
  };
}

describe("physical-scale drawing", () => {
  it("matches the mockup on the 4 sides × 3 alignments", () => {
    for (const side of SIDES) {
      for (const align of ALIGNS) {
        const d = draw({ ...MOCK, arrangement: { aSide: side, align } });
        const want = mockupBoxes(side, align);
        for (const b of d.boxes) {
          const w = want[b.id as keyof typeof want];
          expect(b.left, `${side}/${align}/${b.id}`).toBeCloseTo(w.left, 6);
          expect(b.top).toBeCloseTo(w.top, 6);
          expect(b.width).toBeCloseTo(w.width, 6);
          expect(b.height).toBeCloseTo(w.height, 6);
        }
        expect(d.aspect).toBeCloseTo(76 / 26, 9);
        expect(d.edge?.vertical).toBe(side === "left" || side === "right");
      }
    }
  });

  it("properties: no overlaps, inside the canvas, centered and with the edge between the two", () => {
    const r = rng(117);
    for (let i = 0; i < 300; i++) {
      const sz = () => [4 + r() * 60, 3 + r() * 30] as const;
      const [aw, ah] = sz();
      const [bw, bh] = sz();
      const [cw, ch] = sz();
      const side = SIDES[Math.floor(r() * 4)] ?? "left";
      const align = ALIGNS[Math.floor(r() * 3)] ?? "center";
      const layout: LayoutView = { screens: [screen("a1", "a", aw, ah), screen("b1", "b", bw, bh), screen("x", null, cw, ch, false)], arrangement: { aSide: side, align } };
      const d = draw(layout);
      expect(d.boxes).toHaveLength(3);
      for (const [j, b] of d.boxes.entries()) {
        expect(b.left).toBeGreaterThanOrEqual(-1e-9);
        expect(b.top).toBeGreaterThanOrEqual(-1e-9);
        expect(b.left + b.width).toBeLessThanOrEqual(100 + 1e-9);
        expect(b.top + b.height).toBeLessThanOrEqual(100 + 1e-9);
        for (const c of d.boxes.slice(j + 1)) expect(overlap(b, c), JSON.stringify({ side, align, b, c })).toBe(false);
      }
      // It centers by itself: the same margin on each side on the governing axis.
      const minL = Math.min(...d.boxes.map((b) => b.left));
      const maxR = Math.max(...d.boxes.map((b) => b.left + b.width));
      const minT = Math.min(...d.boxes.map((b) => b.top));
      const maxB = Math.max(...d.boxes.map((b) => b.top + b.height));
      expect(minL).toBeCloseTo(100 - maxR, 6);
      expect(minT).toBeCloseTo(100 - maxB, 6);
      // A ends up on the requested side of B, and the edge is between the two.
      const [a, b] = d.boxes as [Box, Box];
      const e = d.edge;
      expect(e).not.toBeNull();
      if (!e) continue;
      if (side === "left") expect(a.left + a.width).toBeLessThan(b.left);
      if (side === "right") expect(a.left).toBeGreaterThan(b.left + b.width);
      if (side === "up") expect(a.top + a.height).toBeLessThan(b.top);
      if (side === "down") expect(a.top).toBeGreaterThan(b.top + b.height);
      if (e.vertical) {
        expect(e.left).toBeGreaterThan(Math.min(a.left + a.width, b.left + b.width) - 1e-9);
        expect(e.left).toBeLessThan(Math.max(a.left, b.left) + 1e-9);
      } else {
        expect(e.top).toBeGreaterThan(Math.min(a.top + a.height, b.top + b.height) - 1e-9);
        expect(e.top).toBeLessThan(Math.max(a.top, b.top) + 1e-9);
      }
    }
  });

  it("the crossing edge is the shared stretch (`shared_span` of tandem-input), not the length of A", () => {
    // Copy of `tandem_input::geometry::shared_span`: from where to where of B's edge A touches.
    const sharedSpan = (mine: number, theirs: number, align: "start" | "center" | "end") => {
      const off = (mine - theirs) * { start: 0, center: 0.5, end: 1 }[align];
      return [Math.max(off, 0), Math.min(off + theirs, mine)] as const;
    };
    const r = rng(128);
    for (let i = 0; i < 300; i++) {
      const aw = 4 + r() * 60;
      const ah = 3 + r() * 30;
      const bw = 4 + r() * 60;
      const bh = 3 + r() * 30;
      const side = SIDES[Math.floor(r() * 4)] ?? "left";
      const align = ALIGNS[Math.floor(r() * 3)] ?? "center";
      const d = draw({ screens: [screen("a1", "a", aw, ah), screen("b1", "b", bw, bh)], arrangement: { aSide: side, align } });
      const [a, b] = d.boxes as [Box, Box];
      const e = d.edge;
      expect(e).not.toBeNull();
      if (!e) continue;
      const vertical = side === "left" || side === "right";
      // Everything as a percentage of the canvas: converted to inches with B's scale.
      const [bStart, bLen, bTrue, aStart, aLen] = vertical ? [b.top, b.height, bh, a.top, a.height] : [b.left, b.width, bw, a.left, a.width];
      const inch = bTrue / bLen;
      const [from, to] = vertical ? sharedSpan(bh, ah, align) : sharedSpan(bw, aw, align);
      const eStart = vertical ? e.top : e.left;
      expect((eStart - bStart) * inch, `${side}/${align} start`).toBeCloseTo(from, 6);
      expect(e.length * inch, `${side}/${align} length`).toBeCloseTo(to - from, 6);
      // And it stays inside both screens along the edge.
      expect(eStart).toBeGreaterThanOrEqual(Math.max(bStart, aStart) - 1e-9);
      expect(eStart + e.length).toBeLessThanOrEqual(Math.min(bStart + bLen, aStart + aLen) + 1e-9);
    }
  });

  it("the screens of the same machine are drawn together with their x/y", () => {
    // A has two screens side by side, the second 2" lower; B just one, to the right of A.
    const layout: LayoutView = {
      screens: [screen("a1", "a", 10, 8), screen("a2", "a", 10, 8, true, 10, 2), screen("b1", "b", 20, 10), screen("x", null, 6, 5, false)],
      arrangement: { aSide: "left", align: "start" },
    };
    const d = draw(layout);
    const box = (id: string) => d.boxes.find((b) => b.id === id) as Box;
    const [a1, a2, b1, x] = [box("a1"), box("a2"), box("b1"), box("x")];
    // A's touch at the edge and a2 drops 2" (same scale factor as its width: 10" = a1.width).
    expect(a2.left).toBeCloseTo(a1.left + a1.width, 6);
    const inH = a1.height / 8; // height in % per inch
    expect(a2.top - a1.top).toBeCloseTo(2 * inH, 6);
    // A, as a single desktop, is to the left of B; the disconnected one, apart.
    expect(Math.max(a1.left + a1.width, a2.left + a2.width)).toBeLessThan(b1.left);
    for (const [i, p] of d.boxes.entries()) for (const q of d.boxes.slice(i + 1)) expect(overlap(p, q), `${p.id}/${q.id}`).toBe(false);
    expect(x.left).toBeGreaterThan(b1.left + b1.width);
    // The edge is that of A's whole desktop (a1 and a2: from 0 to 10" tall), not that of one screen.
    expect(d.edge).not.toBeNull();
    expect((d.edge?.length ?? 0) / b1.height).toBeCloseTo(10 / 10, 6);
  });

  it("a desk that does not fit enlarges the canvas with the same aspect ratio", () => {
    const big: LayoutView = { screens: [screen("a1", "a", 70, 40), screen("b1", "b", 70, 40)], arrangement: DEFAULT_ARRANGEMENT };
    const d = draw(big);
    expect(d.aspect).toBeCloseTo(76 / 26, 9);
    for (const b of d.boxes) expect(b.left + b.width).toBeLessThanOrEqual(100 + 1e-9);
  });

  it("without the other machine it draws only what exists, without an edge", () => {
    const d = draw({ screens: [screen("a1", "a", 12, 8)], arrangement: DEFAULT_ARRANGEMENT });
    expect(d.boxes).toHaveLength(1);
    expect(d.edge).toBeNull();
    expect(draw({ screens: [], arrangement: DEFAULT_ARRANGEMENT }).boxes).toEqual([]);
  });
});

describe("panel", () => {
  it("with the 34\" chosen it asks about the other machine and shows it on its side", () => {
    const p = panelFor(MOCK, "g34");
    expect(p?.question).toBe("Where is MacBook Pro?");
    expect(p?.other?.id).toBe("mbp");
    expect(p?.shown).toBe("left");
  });

  it("with A's screen chosen the mini plan is seen from A (opposite side)", () => {
    const p = panelFor(MOCK, "mbp");
    expect(p?.question).toBe("Where is the 34\"?");
    expect(p?.shown).toBe("right");
  });

  it("a second connected screen of the same machine is arranged the same as the main one", () => {
    const two: LayoutView = {
      screens: [screen("a1", "a", 10, 8), screen("a2", "a", 10, 8, true, 10, 0), screen("b1", "b", 20, 10)],
      arrangement: { aSide: "left", align: "center" },
    };
    const p = panelFor(two, "a2");
    expect(p?.screen.usable).toBe(true);
    expect(p?.other?.id).toBe("b1");
    expect(p?.question).toBe("Where is b1?");
    expect(p?.shown).toBe("right");
    // What is chosen from the second is the same as from the first.
    expect(placeOther(two.arrangement, "a", "down")).toEqual({ aSide: "up", align: "center" });
  });

  it("a disconnected screen does not take part in the arrangement", () => {
    const p = panelFor(MOCK, "g32");
    expect(p?.other).toBeNull();
    expect(p?.question).toBe("");
    expect(panelFor(MOCK, "nothing")).toBeNull();
  });

  it("putting the other machine in a slot and reading it back gives the same slot, from both", () => {
    for (const m of ["a", "b"] as const) {
      for (const slot of SIDES) {
        const arr = placeOther(DEFAULT_ARRANGEMENT, m, slot);
        const p = panelFor({ ...MOCK, arrangement: arr }, m === "a" ? "mbp" : "g34");
        expect(p?.shown).toBe(slot);
        expect(arr.align).toBe(DEFAULT_ARRANGEMENT.align);
      }
    }
  });

  it("the alignment says Top/Center/Bottom on the sides and Left/Center/Right above and below", () => {
    expect(Object.values(alignLabels("left"))).toEqual(["Top", "Center", "Bottom"]);
    expect(Object.values(alignLabels("right"))).toEqual(["Top", "Center", "Bottom"]);
    expect(Object.values(alignLabels("up"))).toEqual(["Left", "Center", "Right"]);
    expect(Object.values(alignLabels("down"))).toEqual(["Left", "Center", "Right"]);
  });

  it("on opening, the screen of the other machine is chosen", () => {
    expect(initialSelection(MOCK, "a")).toBe("g34");
    expect(initialSelection(MOCK, "b")).toBe("mbp");
    expect(initialSelection({ screens: [], arrangement: DEFAULT_ARRANGEMENT }, "a")).toBeNull();
  });
});

describe("data and colors", () => {
  it("named aspect ratios", () => {
    expect(ratioLabel(3440, 1440)).toBe("21:9");
    expect(ratioLabel(2560, 1440)).toBe("16:9");
    expect(ratioLabel(1920, 1200)).toBe("16:10");
    expect(ratioLabel(1000, 1000)).toBe("1.00:1");
    expect(ratioLabel(0, 10)).toBe("—");
  });

  it("the default accents give the mockup tones and any other one a dark one", () => {
    expect(fillOf("#7aa7e6")).toBe("#16202e");
    expect(fillOf("#A99BE8")).toBe("#1d1a2b");
    expect(fillOf("#6cc59a")).toMatch(/^#[0-9a-f]{6}$/);
    expect(wallOf("#7aa7e6")).toBe("linear-gradient(160deg,#34496a,#1d2738)");
    expect(wallOf("#6cc59a")).toMatch(/^linear-gradient\(160deg,#[0-9a-f]{6},#[0-9a-f]{6}\)$/);
    expect(fillOf("crimson")).toBe("#16171b");
  });

  it("the exit edge of each machine is the opposite of the other's", () => {
    for (const side of SIDES) {
      const arr = { aSide: side, align: "center" as const };
      expect(exitSide(arr, "b")).toBe(side);
      expect(exitSide(arr, "a")).toBe(OPPOSITE[side]);
    }
  });
});

describe("screens at their physical size (T-356)", () => {
  const in_ = (mm: number) => mm / 25.4;
  const wide = { x: 0, y: 0, w: 3440, h: 1440, pw: in_(797), ph: in_(334) };
  const g5 = { x: 3440, y: 0, w: 2560, h: 1440, pw: in_(700), ph: in_(400) };

  it("two monitors side by side touch at their real widths, whatever their density", () => {
    const [a, b] = placePhysical([wide, g5]);
    expect(a).toEqual({ x: 0, y: 0 });
    expect(b!.x).toBeCloseTo(in_(797), 6);
    expect(b!.y).toBe(0);
    // The same order the other way round, and to the left.
    const [c, d] = placePhysical([{ ...g5, x: -2560 }, { ...wide }]);
    expect(d!.x - c!.x).toBeCloseTo(in_(700), 6);
  });

  it("one under another touches at its real height and keeps its offset along the edge", () => {
    const top = { x: 0, y: 0, w: 2560, h: 1440, pw: in_(700), ph: in_(400) };
    const under = { x: 640, y: 1440, w: 1512, h: 982, pw: 12, ph: 7.8 };
    const [, u] = placePhysical([top, under]);
    expect(u!.y).toBeCloseTo(in_(400), 6);
    expect(u!.x).toBeCloseTo(640 * (in_(700) / 2560), 6);
  });

  it("a screen that touches none keeps its native offset, at the first one's scale", () => {
    const far = { x: 5000, y: 0, w: 1920, h: 1080, pw: 20, ph: 11 };
    const [, f] = placePhysical([wide, far]);
    expect(f!.x).toBeCloseTo(5000 * (in_(797) / 3440), 6);
    expect(placePhysical([])).toEqual([]);
  });
});

