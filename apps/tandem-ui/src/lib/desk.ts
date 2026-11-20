// Live desktops of the Desk (`V3-Desk`): the screens at physical scale on the table,
// with their stand, the crossing edge and the cursor. Pure: the geometry comes from the same calculation as the
// arrangement (`draw`), with another canvas and the stands below.

import { draw, mainOf, wallOf, type Drawing, type LayoutScreen, type LayoutView } from "./arrangement";
import type { CursorSample, Machine, Side } from "./model";

/** The mockup canvas measures ≈ 52.3 × 21.6 inches; the 34" (31.4") takes 60 % of the width. */
const DESK_DRAW = { view: { w: 52.3, h: 21.63 }, gap: 1.15, below: 3.67 };

export interface DeskBox {
  id: string;
  machine: Machine | null;
  /** As a percentage of the canvas. */
  left: number;
  top: number;
  width: number;
  height: number;
  /** "MacBook Pro" (built-in) or "34" · PC-Desktop". */
  label: string;
  wall: string;
  /** Its monitor shows neither machine (off, standby, unplugged input): drawn dimmed (T-349). */
  dim?: boolean;
}

/** A piece of the stand (support or base), as a percentage of the canvas. */
export interface Foot {
  left: number;
  top: number;
  width: number;
  height: number;
  /** Radius of the bottom corners, in px. */
  radius: number;
}

export interface DeskScene {
  /** Canvas width / height. */
  aspect: number;
  desks: DeskBox[];
  feet: Foot[];
  /** The edge where the cursor crosses, as a percentage; vertical or horizontal. */
  edge: { vertical: boolean; left: number; top: number; length: number } | null;
}

/** An external monitor has a support stand; a built-in screen, the laptop base. */
export function isExternal(s: Pick<LayoutScreen, "external" | "w">): boolean {
  return s.external ?? s.w > 16;
}

/** How far the laptop base sticks out on each side (fraction of the width) and the stand measurements. */
const LAPTOP_OVER = 0.0655;
const BASE_H = 2.6;
const NECK = { w: 0.075, h: 9 };
const STAND_W = 0.25;
/** The mockup draws the laptop by hand 0.7 % lower and the monitor 0.5 % higher than their measurement. */
const LAPTOP_H = 0.993;
const EXTERNAL_H = 1.005;

/** The desk screens at physical scale, lowered onto the table (with their stands). */
/** The wall of a monitor that shows neither machine: dark and flat, no machine's colour. */
export const DIM_WALL = "#16171b";

export function sceneOf(layout: LayoutView, names: Record<Machine, string>, colors: Record<Machine, string>): DeskScene {
  const usable: LayoutView = { ...layout, screens: layout.screens.filter((s) => s.usable && s.machine !== null) };
  const d: Drawing = draw(usable, DESK_DRAW);
  const byId = new Map(usable.screens.map((s) => [s.id, s]));
  const desks: DeskBox[] = [];
  const feet: Foot[] = [];
  for (const b of d.boxes) {
    const s = byId.get(b.id);
    if (!s?.machine) continue;
    const ext = isExternal(s);
    const who = names[s.machine];
    const height = b.height * (ext ? EXTERNAL_H : LAPTOP_H);
    const top = b.top + b.height - height;
    if (s.dim) {
      desks.push({ id: s.id, machine: s.machine, left: b.left, top, width: b.width, height, label: `${s.short} · ${s.dim}`, wall: DIM_WALL, dim: true });
    } else {
      desks.push({ id: s.id, machine: s.machine, left: b.left, top, width: b.width, height, label: ext ? `${s.short} · ${who}` : who, wall: wallOf(colors[s.machine]) });
    }
    const floor = b.top + b.height;
    const mid = b.left + b.width / 2;
    if (ext) {
      feet.push({ left: mid - (b.width * NECK.w) / 2, top: floor, width: b.width * NECK.w, height: NECK.h, radius: 0 });
      feet.push({ left: mid - (b.width * STAND_W) / 2, top: floor + NECK.h, width: b.width * STAND_W, height: BASE_H, radius: 3 });
    } else {
      feet.push({ left: b.left - b.width * LAPTOP_OVER, top: floor, width: b.width * (1 + 2 * LAPTOP_OVER), height: BASE_H, radius: 6 });
    }
  }
  // The edge falls on B's desktop: on the side where A is.
  let edge: DeskScene["edge"] = null;
  const main = mainOf(usable.screens, "b");
  const box = d.boxes.find((x) => x.id === main?.id);
  if (d.edge && box) {
    const side: Side = layout.arrangement.aSide;
    edge = d.edge.vertical
      ? { vertical: true, left: side === "left" ? box.left : box.left + box.width, top: d.edge.top, length: d.edge.length }
      : { vertical: false, left: d.edge.left, top: side === "up" ? box.top : box.top + box.height, length: d.edge.length };
  }
  return { aspect: d.aspect, desks, feet, edge };
}

export interface Point {
  left: number;
  top: number;
}

/** The desk screen that contains the sample (by its native id, or by the point). */
function screenOf(layout: LayoutView, s: CursorSample): LayoutScreen | null {
  const own = layout.screens.filter((x) => x.machine === s.machine && x.usable && x.native);
  const byId = s.screen ? own.find((x) => x.native?.id === s.screen) : undefined;
  const byPoint = own.find((x) => {
    const n = x.native;
    return n !== undefined && s.x >= n.x && s.x < n.x + n.w && s.y >= n.y && s.y < n.y + n.h;
  });
  return byId ?? byPoint ?? own[0] ?? null;
}

const clamp01 = (v: number) => Math.max(0, Math.min(1, v));

/** Where to draw the cursor (percentage of the canvas); `null` if it is not known which screen it is on. */
export function cursorPoint(scene: DeskScene, layout: LayoutView, s: CursorSample): Point | null {
  const sc = screenOf(layout, s);
  const n = sc?.native;
  const box = scene.desks.find((k) => k.id === sc?.id);
  if (!sc || !n || !box || n.w <= 0 || n.h <= 0) return null;
  return { left: box.left + clamp01((s.x - n.x) / n.w) * box.width, top: box.top + clamp01((s.y - n.y) / n.h) * box.height };
}

/** The opposite of `cursorPoint`: the sample that corresponds to a point of the canvas (for the demo). */
export function sampleAt(scene: DeskScene, layout: LayoutView, p: Point): CursorSample | null {
  const box = scene.desks.find((k) => p.left >= k.left && p.left <= k.left + k.width && p.top >= k.top && p.top <= k.top + k.height);
  const sc = layout.screens.find((x) => x.id === box?.id);
  const n = sc?.native;
  if (!box || !sc?.machine || !n) return null;
  return { machine: sc.machine, screen: n.id, x: n.x + ((p.left - box.left) / box.width) * n.w, y: n.y + ((p.top - box.top) / box.height) * n.h };
}

// ---- Link traffic --------------------------------------------------------------------------

/** Counts the events of the last window to give events per second. */
export class RateMeter {
  private stamps: number[] = [];

  constructor(private readonly windowMs = 1000) {}

  push(now: number): void {
    this.stamps.push(now);
    this.trim(now);
  }

  /** Events per second in the window ending at `now`. */
  rate(now: number): number {
    this.trim(now);
    return (this.stamps.length * 1000) / this.windowMs;
  }

  private trim(now: number): void {
    const from = now - this.windowMs;
    let i = 0;
    while (i < this.stamps.length && (this.stamps[i] ?? 0) <= from) i++;
    if (i > 0) this.stamps.splice(0, i);
  }
}

/**
 * How long the pulse takes to travel a stretch of the link: faster with more traffic. At rest
 * it is slow; with the cursor moving (tens of events per second), at the mockup's speed.
 */
export function flowSeconds(eventsPerSec: number): number {
  if (!(eventsPerSec > 0)) return 1.8;
  if (eventsPerSec < 12) return 1.2;
  return 1;
}

// ---- Demo path (the `cur` keyframes of the mockup) --------------------------------------------

export const DEMO_LOOP_MS = 7000;

/** The points of the path, as a percentage of the canvas, with their instant (0–1). */
const PATH: { t: number; p: Point }[] = [
  { t: 0, p: { left: 16, top: 56 } },
  { t: 0.22, p: { left: 25, top: 52 } },
  { t: 0.38, p: { left: 40, top: 48 } },
  { t: 0.62, p: { left: 63, top: 30 } },
  { t: 0.8, p: { left: 80, top: 55 } },
  { t: 1, p: { left: 16, top: 56 } },
];

/** Smooth when leaving and arriving at each stretch (approximates `cubic-bezier(.45,0,.3,1)`). */
const ease = (u: number) => u * u * (3 - 2 * u);

/** The path point at instant `t` of the cycle (0–1). */
export function demoPath(t: number): Point {
  const x = ((t % 1) + 1) % 1;
  for (let i = 1; i < PATH.length; i++) {
    const a = PATH[i - 1];
    const b = PATH[i];
    if (!a || !b || x > b.t) continue;
    const u = ease((x - a.t) / (b.t - a.t));
    return { left: a.p.left + (b.p.left - a.p.left) * u, top: a.p.top + (b.p.top - a.p.top) * u };
  }
  return PATH[0]?.p ?? { left: 0, top: 0 };
}
