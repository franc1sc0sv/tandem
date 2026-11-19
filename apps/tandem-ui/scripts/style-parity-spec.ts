// Style parity (T-127): which gallery component (`gallery.html`, demo build) is
// compared with which element of which mockup, and the computed-style comparator.
//
// The pixel comparison of `pnpm shots` measures whole screens: a different border, curve or
// duration on a button barely moves pixels (and a transition is not visible in a still
// screenshot). Here the `getComputedStyle` values of the same Chromium are compared, property by
// property. This file does not open the browser, so it is tested with sample styles.

/** Property groups that are compared (the names appear in the report). */
export const GROUPS = {
  height: ["height"],
  width: ["width"],
  padding: ["padding-top", "padding-right", "padding-bottom", "padding-left"],
  font: ["font-family", "font-size", "font-weight", "color"],
  radius: ["border-top-left-radius", "border-top-right-radius", "border-bottom-right-radius", "border-bottom-left-radius"],
  border: ["top", "right", "bottom", "left"].flatMap((s) => [`border-${s}-width`, `border-${s}-style`, `border-${s}-color`]),
  background: ["background-color", "background-image"],
  shadow: ["box-shadow"],
  position: ["top", "left", "bottom"],
  box: ["box-sizing"],
  transition: ["transition-property", "transition-duration", "transition-timing-function", "transition-delay"],
  animation: ["animation-name", "animation-duration", "animation-timing-function", "animation-iteration-count", "animation-fill-mode"],
} as const satisfies Record<string, readonly string[]>;

export type Group = keyof typeof GROUPS;

/** What the task asks for each component: height, padding, font, radius, border, background, shadow and motion. */
export const ALL: Group[] = ["height", "padding", "font", "radius", "border", "background", "shadow", "transition", "animation"];

/** An element (or its `::after`) inside a page. */
export interface Target {
  selector: string;
  pseudo?: "::before" | "::after";
}

export interface StylePair {
    /** Name in the report. */
  name: string;
    /** Element in `gallery.html`. */
  app: Target;
    /** Mockup (without `.dc.html`) and element. */
  mockup: string;
    /** Named mockup state (`tools/mockup-render/src/states.ts`) if the element is not in the initial one. */
  state?: string;
  ref: Target;
  groups: Group[];
}

/** Computed values of an element: CSS property → `getComputedStyle` value. */
export type Styles = Record<string, string>;

export interface StyleDiff {
  pair: string;
  group: Group;
  /** Propiedad CSS (o `transition: transform`, `animation`…). */
  property: string;
  app: string;
  mockup: string;
}

/** All the CSS properties that must be read for the given groups. */
export function propertiesOf(groups: readonly Group[]): string[] {
  return [...new Set(groups.flatMap((g) => GROUPS[g]))];
}

// ---- Normalization ------------------------------------------------------------------------

/** Splits a CSS list on top-level commas (not those of `cubic-bezier(…)` or `rgba(…)`). */
export function splitList(value: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of value) {
    if (ch === "(") depth++;
    else if (ch === ")") depth--;
    if (ch === "," && depth === 0) {
      out.push(cur.trim());
      cur = "";
    } else cur += ch;
  }
  if (cur.trim() !== "" || out.length > 0) out.push(cur.trim());
  return out;
}

/** CSS time in milliseconds (`0.18s`, `180ms`). */
export function toMs(value: string): number {
  const v = value.trim();
  if (v.endsWith("ms")) return Number.parseFloat(v);
  if (v.endsWith("s")) return Number.parseFloat(v) * 1000;
  return Number.parseFloat(v);
}

const NAMED_CURVES: Record<string, string> = {
  linear: "cubic-bezier(0, 0, 1, 1)",
  ease: "cubic-bezier(0.25, 0.1, 0.25, 1)",
  "ease-in": "cubic-bezier(0.42, 0, 1, 1)",
  "ease-out": "cubic-bezier(0, 0, 0.58, 1)",
  "ease-in-out": "cubic-bezier(0.42, 0, 0.58, 1)",
};

/** Curve in canonical form: names become `cubic-bezier` and numbers are rounded. */
export function normalizeCurve(value: string): string {
  const v = value.trim();
  return normalizeNumbers(NAMED_CURVES[v] ?? v);
}

/** Rounds the numbers of a value (Chromium calculations leave `37.9999px`) and joins spaces. */
export function normalizeNumbers(value: string): string {
  return value
    .replace(/-?\d*\.?\d+(?:e-?\d+)?/gi, (n) => {
      const r = Math.round(Number.parseFloat(n) * 100) / 100;
      return String(Object.is(r, -0) ? 0 : r);
    })
    .replace(/\s+/g, " ")
    .trim();
}

/** Only the first family: it is the one that is drawn (the rest is system fallback). */
export function firstFamily(value: string): string {
  return (splitList(value)[0] ?? "").replace(/["']/g, "").trim().toLowerCase();
}

/** Element `i` of a CSS list that repeats if it is shorter (that is how the browser does it). */
function cycle(list: string[], i: number): string {
  return list.length === 0 ? "" : (list[i % list.length] ?? "");
}

/**
  * Transitions of an element as `property → "duration curve delay"`. Those with duration and
  * delay 0 (the initial value `all 0s ease 0s`) animate nothing and are omitted.
 */
export function transitions(s: Styles): Map<string, string> {
  const props = splitList(s["transition-property"] ?? "");
  const durs = splitList(s["transition-duration"] ?? "");
  const curves = splitList(s["transition-timing-function"] ?? "");
  const delays = splitList(s["transition-delay"] ?? "");
  const out = new Map<string, string>();
  props.forEach((p, i) => {
    const d = toMs(cycle(durs, i) || "0s");
    const delay = toMs(cycle(delays, i) || "0s");
    if (p === "none" || (d === 0 && delay === 0)) return;
    out.set(p, `${d}ms ${normalizeCurve(cycle(curves, i) || "ease")}${delay ? ` +${delay}ms` : ""}`);
  });
  return out;
}

/**
  * Animations as a list of `"duration curve repetitions fill"`. The name is not compared
  * (Svelte adds a per-component prefix) nor the delay (cards enter staggered
  * with an online `animation-delay`): only whether there is an animation, its duration and its curve.
 */
export function animations(s: Styles): string[] {
  const names = splitList(s["animation-name"] ?? "none");
  const durs = splitList(s["animation-duration"] ?? "");
  const curves = splitList(s["animation-timing-function"] ?? "");
  const counts = splitList(s["animation-iteration-count"] ?? "");
  const fills = splitList(s["animation-fill-mode"] ?? "");
  return names.flatMap((n, i) =>
    n === "none" ? [] : [`${toMs(cycle(durs, i) || "0s")}ms ${normalizeCurve(cycle(curves, i) || "ease")} ×${cycle(counts, i) || "1"} ${cycle(fills, i) || "none"}`],
  );
}

// ---- Comparison ---------------------------------------------------------------------------

/** Whether the `border-<side>-color` side is drawn: the color of a border that does not exist is not seen. */
function hasBorder(s: Styles, colorProperty: string): boolean {
  const side = colorProperty.replace(/-color$/, "");
  const style = s[`${side}-style`] ?? "none";
  return style !== "none" && style !== "hidden" && Number.parseFloat(s[`${side}-width`] ?? "0") > 0;
}

/** Compares two elements in the requested groups. Returns only what differs. */
export function compareStyles(pair: string, app: Styles, mockup: Styles, groups: readonly Group[]): StyleDiff[] {
  const diffs: StyleDiff[] = [];
  for (const group of groups) {
    if (group === "transition") {
      const a = transitions(app);
      const m = transitions(mockup);
      for (const p of new Set([...a.keys(), ...m.keys()])) {
        if (a.get(p) !== m.get(p)) diffs.push({ pair, group, property: `transition: ${p}`, app: a.get(p) ?? "no transition", mockup: m.get(p) ?? "no transition" });
      }
      continue;
    }
    if (group === "animation") {
      const a = animations(app).join(", ") || "none";
      const m = animations(mockup).join(", ") || "none";
      if (a !== m) diffs.push({ pair, group, property: "animation", app: a, mockup: m });
      continue;
    }
    for (const property of GROUPS[group]) {
      if (group === "border" && property.endsWith("-color") && !hasBorder(app, property) && !hasBorder(mockup, property)) continue;
      const norm = property === "font-family" ? firstFamily : normalizeNumbers;
      const a = norm(app[property] ?? "");
      const m = norm(mockup[property] ?? "");
      if (a !== m) diffs.push({ pair, group, property, app: a, mockup: m });
    }
  }
  return diffs;
}

/** Where each side of a pair is, for the report. */
export function where(p: StylePair): { app: string; mockup: string } {
  const t = (x: Target) => `${x.selector}${x.pseudo ?? ""}`;
  return { app: `gallery.html ${t(p.app)}`, mockup: `${p.mockup}${p.state ? ` (state ${p.state})` : ""} ${t(p.ref)}` };
}

/** Readable report: one line per differing property, with the pair and where each side is. */
export function formatReport(pairs: readonly StylePair[], diffs: readonly StyleDiff[], missing: readonly string[] = []): string {
  const lines: string[] = [];
  for (const p of pairs) {
    const mine = diffs.filter((d) => d.pair === p.name);
    const lost = missing.filter((m) => m.startsWith(`${p.name}:`));
    if (mine.length === 0 && lost.length === 0) {
      lines.push(`✓ ${p.name}`);
      continue;
    }
    const w = where(p);
    lines.push(`✗ ${p.name}  (app: ${w.app} · mockup: ${w.mockup})`);
    for (const l of lost) lines.push(`    ! ${l.slice(p.name.length + 1).trim()}`);
    for (const d of mine) lines.push(`    ${d.group} · ${d.property}: app «${d.app}» ≠ mockup «${d.mockup}»`);
  }
  return lines.join("\n");
}

// ---- Pares --------------------------------------------------------------------------------

const part = (name: string, inner: string, pseudo?: Target["pseudo"]): Target => ({ selector: `[data-part="${name}"] ${inner}`, ...(pseudo ? { pseudo } : {}) });
const ref = (selector: string, pseudo?: Target["pseudo"]): Target => ({ selector, ...(pseudo ? { pseudo } : {}) });

/** What is painted of the primary's shine (`::after`): background, position and its transition. */
const SHINE: Group[] = ["background", "transition"];
/** The switch has no text (the name goes in `aria-label`): the font is not compared. */
const SWITCH: Group[] = [...ALL.filter((g) => g !== "font"), "width"];
/** The switch knob (`::after`). */
const KNOB: Group[] = ["height", "width", "radius", "background", "position", "transition"];
/**
  * The segmented indicator. The width depends on the width of the group (different in the gallery), so
  * the box is looked at (`box-sizing`): with the same formula, the same box gives the same width.
 */
const IND: Group[] = ["height", "box", "radius", "border", "background", "shadow", "position", "transition"];

export const PAIRS: StylePair[] = [
    // V3-Arrangement: the header buttons (38 px).
  { name: "button 38 px", app: part("btn-38", ".btn"), mockup: "V3-Arrangement", ref: ref('a.btn[href="F-CrossingTest.dc.html"]'), groups: ALL },
  { name: "primary button 38 px", app: part("btn-pri-38", ".btn"), mockup: "V3-Arrangement", ref: ref("button.btn.pri"), groups: ALL },
  { name: "primary shine", app: part("btn-pri-38", ".btn", "::after"), mockup: "V3-Arrangement", ref: ref("button.btn.pri", "::after"), groups: SHINE },
    // V3-Monitors: sm, primary sm and icon-only ghost.
  { name: "button sm", app: part("btn-sm", ".btn"), mockup: "V3-Monitors", ref: ref('a.btn.sm[href="V3-States.dc.html"]'), groups: ALL },
  { name: "primary button sm", app: part("btn-pri-sm", ".btn"), mockup: "V3-Monitors", ref: ref("a.btn.pri.sm"), groups: ALL },
  { name: "ghost icon button sm", app: part("btn-ghost-ic-sm", ".btn"), mockup: "V3-Monitors", ref: ref("button.btn.ghost.ic.sm"), groups: [...ALL, "width"] },
    // F-Settings: danger ("Leave desk").
  { name: "danger button", app: part("btn-danger", ".btn"), mockup: "F-Settings", ref: ref("button.btn.danger"), groups: ALL },
    // Segmented: the alignment one of V3-Arrangement (md) and the machine one of V3-Monitors (lg).
  { name: "segmented md", app: part("seg-md", ".seg"), mockup: "V3-Arrangement", ref: ref(".seg"), groups: ["padding", "radius", "border", "background", "shadow"] },
  { name: "segmented md · indicador", app: part("seg-md", ".ind"), mockup: "V3-Arrangement", ref: ref(".seg .ind"), groups: IND },
  { name: "segmented md · selected option", app: part("seg-md", '[aria-checked="true"]'), mockup: "V3-Arrangement", ref: ref('.seg .sb[aria-checked="true"]'), groups: ALL },
  { name: "segmented md · option", app: part("seg-md", '[aria-checked="false"]'), mockup: "V3-Arrangement", ref: ref('.seg .sb[aria-checked="false"]'), groups: ["font"] },
  { name: "segmented lg", app: part("seg-lg", ".seg"), mockup: "V3-Monitors", ref: ref(".seg"), groups: ["padding", "radius", "border", "background", "shadow"] },
  { name: "segmented lg · indicador", app: part("seg-lg", ".ind"), mockup: "V3-Monitors", ref: ref(".seg .ind"), groups: IND },
  { name: "segmented lg · selected option", app: part("seg-lg", '[aria-checked="true"]'), mockup: "V3-Monitors", ref: ref('.seg .segb[aria-checked="true"]'), groups: ALL },
  { name: "segmented lg · option", app: part("seg-lg", '[aria-checked="false"]'), mockup: "V3-Monitors", ref: ref('.seg .segb[aria-checked="false"]'), groups: ["font"] },
    // F-Settings, "Keyboard and mouse" tab: switch on and off, with its knob.
  { name: "switch on", app: part("sw-on", ".sw"), mockup: "F-Settings", state: "keyboard", ref: ref("button.sw:not(.off)"), groups: SWITCH },
  { name: "switch on · knob", app: part("sw-on", ".sw", "::after"), mockup: "F-Settings", state: "keyboard", ref: ref("button.sw:not(.off)", "::after"), groups: KNOB },
  { name: "switch off", app: part("sw-off", ".sw"), mockup: "F-Settings", state: "keyboard", ref: ref("button.sw.off"), groups: SWITCH },
  { name: "switch off · knob", app: part("sw-off", ".sw", "::after"), mockup: "F-Settings", state: "keyboard", ref: ref("button.sw.off", "::after"), groups: KNOB },
    // V3-Desk: the Tunnels count chip and the cards (the desks one and the machine one).
  { name: "count chip", app: part("chip-count", ".chip"), mockup: "V3-Desk", ref: ref(".nav .chip"), groups: ALL },
  { name: "card", app: part("card", ".card"), mockup: "V3-Desk", ref: ref("div.card"), groups: ["padding", "radius", "border", "background", "shadow", "transition", "animation"] },
    // `.mcard` removes the bottom padding for the chart: the rest of the link card is looked at.
  { name: "link card", app: part("card-link", ".card"), mockup: "V3-Desk", ref: ref("a.card.mcard"), groups: ["radius", "border", "background", "shadow", "transition", "animation"] },
];
