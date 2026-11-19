// Arrangement (D18, `V3-Arrangement`): which side of the other machine this one is on and with what
// alignment, without X/Y. This holds the physical-scale drawing (inches), what the panel says
// and the text of the crossing test. Pure: used by the screen, the demo and the tests.

import type { Align, Machine, Side } from "./model";

export type { Align, Side };

/** Which side of B A is on and how they align (what the desk stores, T-099). */
export interface Arrangement {
  aSide: Side;
  align: Align;
}

/** Without a declared arrangement: A to the left of B, centered (`tandem_core::Arrangement`). */
export const DEFAULT_ARRANGEMENT: Arrangement = { aSide: "left", align: "center" };

export const SIDES: Side[] = ["up", "down", "left", "right"];
export const ALIGNS: Align[] = ["start", "center", "end"];

/** A screen of the arrangement, with what the UI shows. */
export interface LayoutScreen {
  id: string;
  /** Which machine it belongs to; `null` if unknown (disconnected). */
  machine: Machine | null;
  /** «Odyssey G5 34"». */
  name: string;
  /** What fits in the mini plan: "34"", "Mac". */
  short: string;
  /** How it is named in a question: "MacBook Pro", "the 34"". */
  ask: string;
  /** Physical size, in inches. */
  w: number;
  h: number;
  /** Where it is on its machine's desktop (`ScreenSpec.x/y`), in inches; the origin does not matter. */
  x: number;
  y: number;
  size: string;
  res: string;
  ratio: string;
  scale: string;
  /** Takes part in the arrangement (connected and compatible). */
  usable: boolean;
  /** The line under the name in the panel ("PC-Desktop · scene Work", "Disconnected"). */
  line: string;
  /** External monitor (with a stand); without data, those wider than 16" are treated as external. */
  external?: boolean;
  /** Its rectangle in the system's native coordinates (those of the `CursorSample` cursor). */
  native?: NativeRect;
}

/** Position and size of a screen in the coordinates of its machine. */
export interface NativeRect {
  /** Native screen id (the one `CursorSample.screen` carries). */
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface LayoutView {
  screens: LayoutScreen[];
  arrangement: Arrangement;
}

export const EMPTY_LAYOUT: LayoutView = { screens: [], arrangement: DEFAULT_ARRANGEMENT };

export const OPPOSITE: Record<Side, Side> = { left: "right", right: "left", up: "down", down: "up" };

/** The edge they share is vertical (left or right). */
export const isVertical = (s: Side): boolean => s === "left" || s === "right";

const FACTOR: Record<Align, number> = { start: 0, center: 0.5, end: 1 };

/** The main screen of each machine: the first one that takes part in the arrangement. */
export function mainOf(screens: LayoutScreen[], m: Machine): LayoutScreen | null {
  return screens.find((s) => s.machine === m && s.usable) ?? null;
}

// ---- Scaled drawing --------------------------------------------------------------------------

/** The canvas measures 76 × 26 inches (like the mockup) and grows if the desk does not fit. */
export const VIEW = { w: 76, h: 26 };
const DEFAULT_VIEW = VIEW;
/** Gap between the two screens that touch and up to those that do not fit, in inches. */
const DEFAULT_GAP = 0.3;
const APART = 1.4;

export interface Box {
  id: string;
  /** As a percentage of the canvas. */
  left: number;
  top: number;
  width: number;
  height: number;
}

/** The crossing edge: a vertical line (`left`, `top`, `height`) or a horizontal one (`width`). */
export interface Edge {
  vertical: boolean;
  left: number;
  top: number;
  /** Length, as a percentage of the width (horizontal) or of the height (vertical). */
  length: number;
}

/** Drawing settings: the canvas, the gap between screens and the space below (stands). */
export interface DrawOptions {
  view?: { w: number; h: number };
  gap?: number;
  /** Free inches under the screens, which count when centering the drawing. */
  below?: number;
}

export interface Drawing {
  boxes: Box[];
  edge: Edge | null;
  /** Canvas aspect ratio (width / height). */
  aspect: number;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** The connected screens of a machine as a single desktop: its box, and each one inside. */
interface Desktop {
  w: number;
  h: number;
  /** Each screen with its position inside the box (origin at its corner). */
  parts: { id: string; x: number; y: number; w: number; h: number }[];
}

function desktopOf(screens: LayoutScreen[], m: Machine): Desktop | null {
  const own = screens.filter((s) => s.machine === m && s.usable);
  if (own.length === 0) return null;
  const x0 = Math.min(...own.map((s) => s.x));
  const y0 = Math.min(...own.map((s) => s.y));
  return {
    w: Math.max(...own.map((s) => s.x + s.w)) - x0,
    h: Math.max(...own.map((s) => s.y + s.h)) - y0,
    parts: own.map((s) => ({ id: s.id, x: s.x - x0, y: s.y - y0, w: s.w, h: s.h })),
  };
}

/**
 * The screens at physical scale, centered on the canvas. Those of each machine go together, with
 * their x/y, like the single desktop the engine sees: B's at the origin, A's beside it
 * according to the arrangement and the rest (those that do not fit) to its right, aligned at the bottom with B's.
 * The crossing edge is the stretch both desktops share (`shared_span` in
 * tandem-input), not the length of either one; it is measured on each desktop's box, which
 * with contiguous screens is what the engine measures.
 */
export function draw(layout: LayoutView, opts: DrawOptions = {}): Drawing {
  const { screens, arrangement } = layout;
  const VIEW = opts.view ?? DEFAULT_VIEW;
  const GAP = opts.gap ?? DEFAULT_GAP;
  const b = desktopOf(screens, "b");
  const a = desktopOf(screens, "a");
  const anchor = b ?? a;
  const rects = new Map<string, Rect>();
  let edge: { vertical: boolean; x: number; y: number; len: number } | null = null;
  const put = (d: Desktop, ox: number, oy: number) => {
    for (const p of d.parts) rects.set(p.id, { x: ox + p.x, y: oy + p.y, w: p.w, h: p.h });
  };
  if (anchor) {
    const S = { w: anchor.w, h: anchor.h };
    put(anchor, 0, 0);
    if (a && b) {
      const k = FACTOR[arrangement.align];
      const side = arrangement.aSide;
      let mx: number;
      let my: number;
      if (isVertical(side)) {
        mx = side === "left" ? -a.w - GAP : S.w + GAP;
        my = (S.h - a.h) * k;
      } else {
        my = side === "up" ? -a.h - GAP : S.h + GAP;
        mx = (S.w - a.w) * k;
      }
      put(a, mx, my);
      // The shared stretch: from max(A's start, 0) to min(A's end, B's length), along the edge axis.
      const vertical = isVertical(side);
      const from = Math.max(vertical ? my : mx, 0);
      const to = Math.min(vertical ? my + a.h : mx + a.w, vertical ? S.h : S.w);
      if (to > from) {
        const across = side === "left" ? mx + a.w + GAP / 2 : side === "up" ? my + a.h + GAP / 2 : vertical ? S.w + GAP / 2 : S.h + GAP / 2;
        edge = vertical ? { vertical, x: across, y: from, len: to - from } : { vertical, x: from, y: across, len: to - from };
      }
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
  const maxy = Math.max(...all.map((r) => r.y + r.h)) + (opts.below ?? 0);
  // If the desk does not fit in 76 × 26, the canvas grows with the same aspect ratio.
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

/** The mini plan slots, as a percentage (those of the mockup). */
export const SLOTS: Record<Side, { left: number; top: number; width: number; height: number }> = {
  up: { left: 40, top: 7, width: 20, height: 22 },
  down: { left: 40, top: 71, width: 20, height: 22 },
  left: { left: 6, top: 37, width: 20, height: 26 },
  right: { left: 74, top: 37, width: 20, height: 26 },
};

export const SIDE_LABEL: Record<Side, string> = { up: "Top", down: "Bottom", left: "Left", right: "Right" };

/** The alignment labels: on the sides it aligns vertically, above and below horizontally. */
export function alignLabels(side: Side): Record<Align, string> {
  return isVertical(side) ? { start: "Top", center: "Center", end: "Bottom" } : { start: "Left", center: "Center", end: "Right" };
}

export interface Panel {
  screen: LayoutScreen;
  /** The main screen of the other machine (the one that moves in the mini plan), if it can be edited. */
  other: LayoutScreen | null;
  /** Where `other` is relative to the chosen one. */
  shown: Side;
  /** "Where is MacBook Pro?". */
  question: string;
}

/** What the panel shows for the chosen screen. */
export function panelFor(layout: LayoutView, selected: string): Panel | null {
  const screen = layout.screens.find((s) => s.id === selected);
  if (!screen) return null;
  const m = screen.machine;
  const other = m ? mainOf(layout.screens, m === "a" ? "b" : "a") : null;
  // All the connected screens of a machine edit the same arrangement.
  const editable = screen.usable && m !== null && other !== null;
  const aSide = layout.arrangement.aSide;
  // `aSide` is where A is seen from B: from A, B is on the opposite side.
  const shown = m === "a" ? OPPOSITE[aSide] : aSide;
  return { screen, other: editable ? other : null, shown, question: editable && other ? `Where is ${other.ask}?` : "" };
}

/** Put the other machine in `slot` of the mini plan of `machine`'s screen. */
export function placeOther(arr: Arrangement, machine: Machine, slot: Side): Arrangement {
  return { ...arr, aSide: machine === "a" ? OPPOSITE[slot] : slot };
}

/** The screen chosen on opening: the main one of the other machine (the one usually moved around). */
export function initialSelection(layout: LayoutView, here: Machine): string | null {
  const peer = mainOf(layout.screens, here === "a" ? "b" : "a");
  return peer?.id ?? mainOf(layout.screens, here)?.id ?? layout.screens[0]?.id ?? null;
}

// ---- Data of a screen ----------------------------------------------------------------------

const RATIOS: [number, number][] = [
  [16, 10],
  [16, 9],
  [21, 9],
  [32, 9],
  [4, 3],
  [3, 2],
  [5, 4],
];

/** "16:9", "21:9" (3440 × 1440 is sold as 21:9) or, if it resembles none, "1.85:1". */
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

/** Logical points per inch assumed when the screen does not report its physical size. */
export const ASSUMED_PPI = 110;

// ---- Colors --------------------------------------------------------------------------------

function hex(c: string): [number, number, number] | null {
  const m = /^#([0-9a-f]{6})$/i.exec(c);
  if (!m?.[1]) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

const toHex = (rgb: number[]) => `#${rgb.map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0")).join("")}`;

// The two default accents have the exact tones of the mockup; the rest is computed the same way.
const FILL: Record<string, string> = { "#7aa7e6": "#16202e", "#a99be8": "#1d1a2b" };
const WALL: Record<string, [string, string]> = { "#7aa7e6": ["#34496a", "#1d2738"], "#a99be8": ["#463d63", "#241f33"] };

/** A screen's background on the canvas: its machine's accent, very dark. */
export function fillOf(color: string): string {
  const known = FILL[color.toLowerCase()];
  if (known) return known;
  const rgb = hex(color);
  return rgb ? toHex(rgb.map((v) => v * 0.19)) : "#16171b";
}

/** The screen background of the panel thumbnail. */
export function wallOf(color: string): string {
  const known = WALL[color.toLowerCase()];
  const rgb = hex(color);
  const [from, to] = known ?? (rgb ? [toHex(rgb.map((v) => v * 0.42)), toHex(rgb.map((v) => v * 0.22))] : ["#2a2c33", "#18191d"]);
  return `linear-gradient(160deg,${from},${to})`;
}

// ---- Crossing test -----------------------------------------------------------------------

/** Which way the cursor has to leave `from` to reach the other machine. */
export function exitSide(arr: Arrangement, from: Machine): Side {
  // `aSide` is the side of B where A is: from B you leave through `aSide`, from A through the opposite.
  return from === "b" ? arr.aSide : OPPOSITE[arr.aSide];
}

/** "right of", "above". */
export const SIDE_PHRASE: Record<Side, string> = { right: "to the right of", left: "to the left of", up: "above", down: "below" };
/** «the right», «the top». */
export const SIDE_BY: Record<Side, string> = { right: "the right", left: "the left", up: "the top", down: "the bottom" };
