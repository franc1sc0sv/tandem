import { describe, expect, it } from "vitest";

import { ALIGNS, DEFAULT_ARRANGEMENT, OPPOSITE, SIDES, alignLabels, draw, exitSide, fillOf, initialSelection, panelFor, placeOther, ratioLabel, wallOf, type Box, type LayoutScreen, type LayoutView } from "./acomodo";
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

/** Lo que calcula el mockup (`V3-Acomodo`, `renderVals`) para el Mac, el 34" y el 32". */
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

/** Generador con semilla: las mismas pruebas de propiedades en cada corrida. */
function rng(seed: number) {
  let s = seed;
  return () => {
    s = (s * 1664525 + 1013904223) % 4294967296;
    return s / 4294967296;
  };
}

describe("dibujo a escala física", () => {
  it("coincide con el mockup en los 4 lados × 3 alineaciones", () => {
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

  it("propiedades: sin solapes, dentro del lienzo, centrado y con el borde entre las dos", () => {
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
      // Se centra solo: el mismo margen a cada lado en el eje que manda.
      const minL = Math.min(...d.boxes.map((b) => b.left));
      const maxR = Math.max(...d.boxes.map((b) => b.left + b.width));
      const minT = Math.min(...d.boxes.map((b) => b.top));
      const maxB = Math.max(...d.boxes.map((b) => b.top + b.height));
      expect(minL).toBeCloseTo(100 - maxR, 6);
      expect(minT).toBeCloseTo(100 - maxB, 6);
      // A queda del lado pedido de B, y el borde está entre las dos.
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

  it("el borde de cruce es el tramo compartido (`shared_span` de tandem-input), no el largo de A", () => {
    // Copia de `tandem_input::geometry::shared_span`: desde dónde hasta dónde del borde de B toca A.
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
      // Todo en porcentaje del lienzo: se pasa a pulgadas con la escala de B.
      const [bStart, bLen, bTrue, aStart, aLen] = vertical ? [b.top, b.height, bh, a.top, a.height] : [b.left, b.width, bw, a.left, a.width];
      const inch = bTrue / bLen;
      const [from, to] = vertical ? sharedSpan(bh, ah, align) : sharedSpan(bw, aw, align);
      const eStart = vertical ? e.top : e.left;
      expect((eStart - bStart) * inch, `${side}/${align} inicio`).toBeCloseTo(from, 6);
      expect(e.length * inch, `${side}/${align} largo`).toBeCloseTo(to - from, 6);
      // Y queda dentro de las dos pantallas a lo largo del borde.
      expect(eStart).toBeGreaterThanOrEqual(Math.max(bStart, aStart) - 1e-9);
      expect(eStart + e.length).toBeLessThanOrEqual(Math.min(bStart + bLen, aStart + aLen) + 1e-9);
    }
  });

  it("las pantallas de una misma máquina se dibujan juntas con su x/y", () => {
    // A tiene dos pantallas pegadas, la segunda 2" más abajo; B una sola, a la derecha de A.
    const layout: LayoutView = {
      screens: [screen("a1", "a", 10, 8), screen("a2", "a", 10, 8, true, 10, 2), screen("b1", "b", 20, 10), screen("x", null, 6, 5, false)],
      arrangement: { aSide: "left", align: "start" },
    };
    const d = draw(layout);
    const box = (id: string) => d.boxes.find((b) => b.id === id) as Box;
    const [a1, a2, b1, x] = [box("a1"), box("a2"), box("b1"), box("x")];
    // Las de A se tocan en el borde y a2 baja 2" (mismo factor de escala que su ancho: 10" = a1.width).
    expect(a2.left).toBeCloseTo(a1.left + a1.width, 6);
    const inH = a1.height / 8; // alto en % por pulgada
    expect(a2.top - a1.top).toBeCloseTo(2 * inH, 6);
    // A, como un solo escritorio, queda a la izquierda de B; la desconectada, aparte.
    expect(Math.max(a1.left + a1.width, a2.left + a2.width)).toBeLessThan(b1.left);
    for (const [i, p] of d.boxes.entries()) for (const q of d.boxes.slice(i + 1)) expect(overlap(p, q), `${p.id}/${q.id}`).toBe(false);
    expect(x.left).toBeGreaterThan(b1.left + b1.width);
    // El borde es el de todo el escritorio de A (a1 y a2: de 0 a 10" de alto), no el de una pantalla.
    expect(d.edge).not.toBeNull();
    expect((d.edge?.length ?? 0) / b1.height).toBeCloseTo(10 / 10, 6);
  });

  it("un puesto que no entra agranda el lienzo con la misma proporción", () => {
    const big: LayoutView = { screens: [screen("a1", "a", 70, 40), screen("b1", "b", 70, 40)], arrangement: DEFAULT_ARRANGEMENT };
    const d = draw(big);
    expect(d.aspect).toBeCloseTo(76 / 26, 9);
    for (const b of d.boxes) expect(b.left + b.width).toBeLessThanOrEqual(100 + 1e-9);
  });

  it("sin la otra máquina dibuja solo lo que hay, sin borde", () => {
    const d = draw({ screens: [screen("a1", "a", 12, 8)], arrangement: DEFAULT_ARRANGEMENT });
    expect(d.boxes).toHaveLength(1);
    expect(d.edge).toBeNull();
    expect(draw({ screens: [], arrangement: DEFAULT_ARRANGEMENT }).boxes).toEqual([]);
  });
});

describe("panel", () => {
  it("con el 34\" elegido pregunta por la otra máquina y la muestra de su lado", () => {
    const p = panelFor(MOCK, "g34");
    expect(p?.question).toBe("¿Dónde está MacBook Pro?");
    expect(p?.other?.id).toBe("mbp");
    expect(p?.shown).toBe("left");
  });

  it("con la pantalla de A elegida el mini plano se ve desde A (lado opuesto)", () => {
    const p = panelFor(MOCK, "mbp");
    expect(p?.question).toBe('¿Dónde está el 34"?');
    expect(p?.shown).toBe("right");
  });

  it("una segunda pantalla conectada de la misma máquina se acomoda igual que la principal", () => {
    const two: LayoutView = {
      screens: [screen("a1", "a", 10, 8), screen("a2", "a", 10, 8, true, 10, 0), screen("b1", "b", 20, 10)],
      arrangement: { aSide: "left", align: "center" },
    };
    const p = panelFor(two, "a2");
    expect(p?.screen.usable).toBe(true);
    expect(p?.other?.id).toBe("b1");
    expect(p?.question).toBe("¿Dónde está b1?");
    expect(p?.shown).toBe("right");
    // Lo que se elige desde la segunda es lo mismo que desde la primera.
    expect(placeOther(two.arrangement, "a", "down")).toEqual({ aSide: "up", align: "center" });
  });

  it("una pantalla desconectada no entra en el acomodo", () => {
    const p = panelFor(MOCK, "g32");
    expect(p?.other).toBeNull();
    expect(p?.question).toBe("");
    expect(panelFor(MOCK, "nada")).toBeNull();
  });

  it("poner la otra máquina en una casilla y volver a leerla da la misma casilla, desde las dos", () => {
    for (const m of ["a", "b"] as const) {
      for (const slot of SIDES) {
        const arr = placeOther(DEFAULT_ARRANGEMENT, m, slot);
        const p = panelFor({ ...MOCK, arrangement: arr }, m === "a" ? "mbp" : "g34");
        expect(p?.shown).toBe(slot);
        expect(arr.align).toBe(DEFAULT_ARRANGEMENT.align);
      }
    }
  });

  it("la alineación dice Arriba/Centro/Abajo a los lados e Izquierda/Centro/Derecha arriba y abajo", () => {
    expect(Object.values(alignLabels("left"))).toEqual(["Arriba", "Centro", "Abajo"]);
    expect(Object.values(alignLabels("right"))).toEqual(["Arriba", "Centro", "Abajo"]);
    expect(Object.values(alignLabels("up"))).toEqual(["Izquierda", "Centro", "Derecha"]);
    expect(Object.values(alignLabels("down"))).toEqual(["Izquierda", "Centro", "Derecha"]);
  });

  it("al abrir se elige la pantalla de la otra máquina", () => {
    expect(initialSelection(MOCK, "a")).toBe("g34");
    expect(initialSelection(MOCK, "b")).toBe("mbp");
    expect(initialSelection({ screens: [], arrangement: DEFAULT_ARRANGEMENT }, "a")).toBeNull();
  });
});

describe("datos y colores", () => {
  it("proporciones con nombre", () => {
    expect(ratioLabel(3440, 1440)).toBe("21:9");
    expect(ratioLabel(2560, 1440)).toBe("16:9");
    expect(ratioLabel(1920, 1200)).toBe("16:10");
    expect(ratioLabel(1000, 1000)).toBe("1.00:1");
    expect(ratioLabel(0, 10)).toBe("—");
  });

  it("los acentos por omisión dan los tonos del mockup y cualquier otro uno oscuro", () => {
    expect(fillOf("#7aa7e6")).toBe("#16202e");
    expect(fillOf("#A99BE8")).toBe("#1d1a2b");
    expect(fillOf("#6cc59a")).toMatch(/^#[0-9a-f]{6}$/);
    expect(wallOf("#7aa7e6")).toBe("linear-gradient(160deg,#34496a,#1d2738)");
    expect(wallOf("#6cc59a")).toMatch(/^linear-gradient\(160deg,#[0-9a-f]{6},#[0-9a-f]{6}\)$/);
    expect(fillOf("rojo")).toBe("#16171b");
  });

  it("el borde de salida de cada máquina es el opuesto del de la otra", () => {
    for (const side of SIDES) {
      const arr = { aSide: side, align: "center" as const };
      expect(exitSide(arr, "b")).toBe(side);
      expect(exitSide(arr, "a")).toBe(OPPOSITE[side]);
    }
  });
});
