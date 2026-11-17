// Acomodo (D18, `V3-Acomodo`): de qué lado de la otra máquina está esta y con qué
// alineación, sin X/Y. Aquí está el dibujo a escala física (pulgadas), lo que dice el panel
// y el texto de la prueba de cruce. Puro: lo usan la pantalla, el demo y las pruebas.

import type { Align, Machine, Side } from "./model";

export type { Align, Side };

/** De qué lado de B está A y cómo se alinean (lo que guarda el puesto, T-099). */
export interface Arrangement {
  aSide: Side;
  align: Align;
}

/** Sin acomodo declarado: A a la izquierda de B, centradas (`tandem_core::Arrangement`). */
export const DEFAULT_ARRANGEMENT: Arrangement = { aSide: "left", align: "center" };

export const SIDES: Side[] = ["up", "down", "left", "right"];
export const ALIGNS: Align[] = ["start", "center", "end"];

/** Una pantalla del acomodo, ya con lo que muestra la UI. */
export interface LayoutScreen {
  id: string;
  /** De qué máquina es; `null` si no se sabe (desconectada). */
  machine: Machine | null;
  /** «Odyssey G5 34"». */
  name: string;
  /** Lo que cabe en el mini plano: «34"», «Mac». */
  short: string;
  /** Cómo se la nombra en una pregunta: «MacBook Pro», «el 34"». */
  ask: string;
  /** Tamaño físico, en pulgadas. */
  w: number;
  h: number;
  size: string;
  res: string;
  ratio: string;
  scale: string;
  /** Entra en el acomodo (conectada y compatible). */
  usable: boolean;
  /** La línea bajo el nombre en el panel («PC-Escritorio · escena Trabajo», «Desconectado»). */
  line: string;
}

export interface LayoutView {
  screens: LayoutScreen[];
  arrangement: Arrangement;
}

export const EMPTY_LAYOUT: LayoutView = { screens: [], arrangement: DEFAULT_ARRANGEMENT };

export const OPPOSITE: Record<Side, Side> = { left: "right", right: "left", up: "down", down: "up" };

/** El borde que comparten es vertical (izquierda o derecha). */
export const isVertical = (s: Side): boolean => s === "left" || s === "right";

const FACTOR: Record<Align, number> = { start: 0, center: 0.5, end: 1 };

/** La pantalla principal de cada máquina: la primera que entra en el acomodo. */
export function mainOf(screens: LayoutScreen[], m: Machine): LayoutScreen | null {
  return screens.find((s) => s.machine === m && s.usable) ?? null;
}

// ---- Dibujo a escala -------------------------------------------------------------------------

/** El lienzo mide 76 × 26 pulgadas (como el mockup) y crece si el puesto no entra. */
export const VIEW = { w: 76, h: 26 };
/** Separación entre las dos pantallas que se tocan y hasta las que no entran, en pulgadas. */
const GAP = 0.3;
const APART = 1.4;

export interface Box {
  id: string;
  /** En porcentaje del lienzo. */
  left: number;
  top: number;
  width: number;
  height: number;
}

/** El borde de cruce: una línea vertical (`left`, `top`, `height`) u horizontal (`width`). */
export interface Edge {
  vertical: boolean;
  left: number;
  top: number;
  /** Largo, en porcentaje del ancho (horizontal) o del alto (vertical). */
  length: number;
}

export interface Drawing {
  boxes: Box[];
  edge: Edge | null;
  /** Proporción del lienzo (ancho / alto). */
  aspect: number;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Las pantallas a escala física, centradas en el lienzo: la principal de B en el origen, la
 * de A a su lado según el acomodo y el resto (las que no entran) a la derecha, alineadas
 * abajo con la de B.
 */
export function draw(layout: LayoutView): Drawing {
  const { screens, arrangement } = layout;
  const b = mainOf(screens, "b");
  const a = mainOf(screens, "a");
  const anchor = b ?? a;
  const rects = new Map<string, Rect>();
  let edge: { vertical: boolean; x: number; y: number; len: number } | null = null;
  if (anchor) {
    const S = { x: 0, y: 0, w: anchor.w, h: anchor.h };
    rects.set(anchor.id, S);
    if (a && b) {
      const k = FACTOR[arrangement.align];
      const side = arrangement.aSide;
      const M = { x: 0, y: 0, w: a.w, h: a.h };
      if (isVertical(side)) {
        M.x = side === "left" ? -M.w - GAP : S.w + GAP;
        M.y = (S.h - M.h) * k;
      } else {
        M.y = side === "up" ? -M.h - GAP : S.h + GAP;
        M.x = (S.w - M.w) * k;
      }
      rects.set(a.id, M);
      // El tramo que comparten: el de la más corta a lo largo del borde (como el mockup).
      if (side === "left") edge = { vertical: true, x: M.x + M.w + GAP / 2, y: M.y, len: M.h };
      else if (side === "right") edge = { vertical: true, x: S.w + GAP / 2, y: M.y, len: M.h };
      else if (side === "up") edge = { vertical: false, x: M.x, y: M.y + M.h + GAP / 2, len: M.w };
      else edge = { vertical: false, x: M.x, y: S.h + GAP / 2, len: M.w };
    }
    let x = Math.max(...[...rects.values()].map((r) => r.x + r.w)) + APART;
    for (const s of screens) {
      if (rects.has(s.id)) continue;
      rects.set(s.id, { x, y: S.h - s.h, w: s.w, h: s.h });
      x += s.w + APART;
    }
  }
  const all = [...rects.values()];
  if (all.length === 0) return { boxes: [], edge: null, aspect: VIEW.w / VIEW.h };
  const minx = Math.min(0, ...all.map((r) => r.x));
  const maxx = Math.max(...all.map((r) => r.x + r.w));
  const miny = Math.min(0, ...all.map((r) => r.y));
  const maxy = Math.max(...all.map((r) => r.y + r.h));
  // Si el puesto no entra en 76 × 26, el lienzo crece con la misma proporción.
  const grow = Math.max(1, (maxx - minx) / VIEW.w, (maxy - miny) / VIEW.h);
  const W = VIEW.w * grow;
  const H = VIEW.h * grow;
  const ox = (W - (maxx - minx)) / 2 - minx;
  const oy = (H - (maxy - miny)) / 2 - miny;
  const boxes = screens.flatMap((s) => {
    const r = rects.get(s.id);
    return r ? [{ id: s.id, left: ((r.x + ox) / W) * 100, top: ((r.y + oy) / H) * 100, width: (r.w / W) * 100, height: (r.h / H) * 100 }] : [];
  });
  const e = edge ? { vertical: edge.vertical, left: ((edge.x + ox) / W) * 100, top: ((edge.y + oy) / H) * 100, length: (edge.len / (edge.vertical ? H : W)) * 100 } : null;
  return { boxes, edge: e, aspect: W / H };
}

// ---- Panel ---------------------------------------------------------------------------------

/** Las casillas del mini plano, en porcentaje (las del mockup). */
export const SLOTS: Record<Side, { left: number; top: number; width: number; height: number }> = {
  up: { left: 40, top: 7, width: 20, height: 22 },
  down: { left: 40, top: 71, width: 20, height: 22 },
  left: { left: 6, top: 37, width: 20, height: 26 },
  right: { left: 74, top: 37, width: 20, height: 26 },
};

export const SIDE_LABEL: Record<Side, string> = { up: "Arriba", down: "Abajo", left: "Izquierda", right: "Derecha" };

/** Las etiquetas de la alineación: a los lados se alinea en vertical, arriba y abajo en horizontal. */
export function alignLabels(side: Side): Record<Align, string> {
  return isVertical(side) ? { start: "Arriba", center: "Centro", end: "Abajo" } : { start: "Izquierda", center: "Centro", end: "Derecha" };
}

export interface Panel {
  screen: LayoutScreen;
  /** La otra pantalla principal (la que se mueve en el mini plano), si se puede editar. */
  other: LayoutScreen | null;
  /** Dónde está `other` respecto de la elegida. */
  shown: Side;
  /** «¿Dónde está MacBook Pro?». */
  question: string;
}

/** Lo que muestra el panel para la pantalla elegida. Solo las principales se acomodan. */
export function panelFor(layout: LayoutView, selected: string): Panel | null {
  const screen = layout.screens.find((s) => s.id === selected);
  if (!screen) return null;
  const m = screen.machine;
  const own = m ? mainOf(layout.screens, m) : null;
  const other = m ? mainOf(layout.screens, m === "a" ? "b" : "a") : null;
  const editable = own?.id === screen.id && other !== null;
  const aSide = layout.arrangement.aSide;
  // `aSide` es dónde está A visto desde B: desde A, B queda del lado opuesto.
  const shown = m === "a" ? OPPOSITE[aSide] : aSide;
  return { screen, other: editable ? other : null, shown, question: editable && other ? `¿Dónde está ${other.ask}?` : "" };
}

/** Poner la otra máquina en `slot` del mini plano de la pantalla de `machine`. */
export function placeOther(arr: Arrangement, machine: Machine, slot: Side): Arrangement {
  return { ...arr, aSide: machine === "a" ? OPPOSITE[slot] : slot };
}

/** La pantalla que se elige al abrir: la principal de la otra máquina (la que se suele mover alrededor). */
export function initialSelection(layout: LayoutView, here: Machine): string | null {
  const peer = mainOf(layout.screens, here === "a" ? "b" : "a");
  return peer?.id ?? mainOf(layout.screens, here)?.id ?? layout.screens[0]?.id ?? null;
}

// ---- Datos de una pantalla -----------------------------------------------------------------

const RATIOS: [number, number][] = [
  [16, 10],
  [16, 9],
  [21, 9],
  [32, 9],
  [4, 3],
  [3, 2],
  [5, 4],
];

/** «16:9», «21:9» (3440 × 1440 se vende como 21:9) o, si no se parece a ninguna, «1.85:1». */
export function ratioLabel(w: number, h: number): string {
  if (!(w > 0 && h > 0)) return "—";
  const r = w / h;
  let best: [number, number] | null = null;
  let err = Infinity;
  for (const [x, y] of RATIOS) {
    const e = Math.abs(r - x / y) / (x / y);
    if (e < err) {
      err = e;
      best = [x, y];
    }
  }
  return best && err < 0.05 ? `${best[0]}:${best[1]}` : `${r.toFixed(2)}:1`;
}

/** Puntos lógicos por pulgada que se suponen cuando la pantalla no dice su tamaño físico. */
export const ASSUMED_PPI = 110;

// ---- Colores -------------------------------------------------------------------------------

function hex(c: string): [number, number, number] | null {
  const m = /^#([0-9a-f]{6})$/i.exec(c);
  if (!m?.[1]) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

const toHex = (rgb: number[]) => `#${rgb.map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0")).join("")}`;

// Los dos acentos por omisión tienen los tonos exactos del mockup; el resto se calcula igual.
const FILL: Record<string, string> = { "#7aa7e6": "#16202e", "#a99be8": "#1d1a2b" };
const WALL: Record<string, [string, string]> = { "#7aa7e6": ["#34496a", "#1d2738"], "#a99be8": ["#463d63", "#241f33"] };

/** El fondo de una pantalla en el lienzo: el acento de su máquina, muy oscuro. */
export function fillOf(color: string): string {
  const known = FILL[color.toLowerCase()];
  if (known) return known;
  const rgb = hex(color);
  return rgb ? toHex(rgb.map((v) => v * 0.19)) : "#16171b";
}

/** El fondo de pantalla de la miniatura del panel. */
export function wallOf(color: string): string {
  const known = WALL[color.toLowerCase()];
  const rgb = hex(color);
  const [from, to] = known ?? (rgb ? [toHex(rgb.map((v) => v * 0.42)), toHex(rgb.map((v) => v * 0.22))] : ["#2a2c33", "#18191d"]);
  return `linear-gradient(160deg,${from},${to})`;
}

// ---- Prueba de cruce -----------------------------------------------------------------------

/** Por dónde tiene que salir el cursor de `from` para llegar a la otra máquina. */
export function exitSide(arr: Arrangement, from: Machine): Side {
  // `aSide` es el lado de B donde está A: desde B se sale por `aSide`, desde A por el opuesto.
  return from === "b" ? arr.aSide : OPPOSITE[arr.aSide];
}

/** «a la derecha», «arriba». */
export const SIDE_PHRASE: Record<Side, string> = { right: "a la derecha", left: "a la izquierda", up: "arriba", down: "abajo" };
/** «por la derecha», «por arriba». */
export const SIDE_BY: Record<Side, string> = { right: "por la derecha", left: "por la izquierda", up: "por arriba", down: "por abajo" };
