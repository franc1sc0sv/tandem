import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { setReducedMotion } from "../test/setup";
import { reducedMotion } from "../lib/motion";

// G7 and G8 for Monitors, Scene and Progress (`V3-Monitors`, `F-NewScene`, `F-Transactions`).
// vitest runs from apps/tandem-ui.
const SRC = join(process.cwd(), "src");
const read = (...p: string[]) => readFileSync(join(SRC, ...p), "utf8");
const mockup = (name: string) => readFileSync(join(process.cwd(), "..", "..", "mockups", "v03", `${name}.dc.html`), "utf8");

/** `.45s` -> `0.45s`, `cubic-bezier(.2,.8,.2,1)` -> `cubic-bezier(0.2, 0.8, 0.2, 1)`: as the app's CSS writes it. */
const canon = (css: string) =>
  css
    .replace(/(^|[^\d])\.(\d)/g, "$10.$2")
    .replace(/,\s*/g, ", ")
    .replace(/\s+/g, " ")
    .trim();

/** The mockup's `animation:` declarations (without the reduced-motion one) in canonical form. */
const animationsOf = (html: string) => [...html.matchAll(/animation:([^;}"]*)/g)].map((m) => m[1]).filter((v) => !v.includes("none")).map(canon);
const keyframesOf = (html: string) => [...html.matchAll(/@keyframes (\w+)/g)].map((m) => m[1]).sort();

/** The CSS of the global `prefers-reduced-motion` rule. */
const GLOBAL = read("styles", "global.css");
const RULE = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(GLOBAL)?.[1] ?? "";

/** Duration and curve of each mockup animation (without the name, which changes with the screen's prefix). */
const timing = (decl: string) => decl.replace(/^\w+ /, "");

const SCREENS = {
  monitors: {
    mock: mockup("V3-Monitors"),
    // `up`, `in` and `pl` live in the screen; `fls` (the thumbnail flash) in `Thumb`.
    files: [read("screens", "Monitors.svelte"), read("ui", "Thumb.svelte")],
    keyframes: ["mon-up", "mon-in", "mon-pl", "thumb-fls"],
    skip: ["dr"],
  },
  scene: {
    mock: mockup("F-NewScene"),
    files: [read("screens", "Scene.svelte")],
    keyframes: ["es-bd"],
    skip: [],
  },
  tunnels: {
    mock: mockup("V3-Compute"),
    // The sidebar dot pulse (`pl`) lives in `Sidebar`; that of the counters, in the screen.
    files: [read("screens", "Tunnels.svelte"), read("shell", "Sidebar.svelte")],
    keyframes: ["tu-up", "tu-dn", "tu-bar", "tu-sp", "tu-mv", "tu-fa", "tu-fb", "tu-halo", "tu-pl", "tu-bl"],
    // `nw` (the flash of a new event) is declared by the mockup for a list it does not draw; `pl` is
    // tested separately (the mockup's inline declaration cannot be read as CSS).
    skip: ["nw", "pl"],
  },
  progress: {
    mock: mockup("F-Transactions"),
    files: [read("screens", "Progress.svelte")],
    keyframes: ["pr-sp", "pr-wv", "pr-fl", "pr-up"],
    skip: ["cnt"],
  },
} as const;

describe.each(Object.entries(SCREENS))("animations of %s", (_name, s) => {
  // The screen and the shared components that draw part of the mockup (card, thumbnail, selector, button).
  const css = [...s.files, read("ui", "Card.svelte"), read("ui", "Thumb.svelte"), read("ui", "Segmented.svelte"), read("ui", "Button.svelte")].join("\n");
  const has = (text: string, what: string) => expect(canon(css).includes(text), `missing "${what}"`).toBe(true);

  it("each mockup @keyframes has its equivalent", () => {
    const names = keyframesOf(s.mock).filter((k) => !(s.skip as readonly string[]).includes(k));
    // One equivalent for each of the mockup's (except those the mockup declares and does not use).
    expect(s.keyframes.length).toBeGreaterThanOrEqual(names.length);
    for (const k of s.keyframes) expect(css.includes(`@keyframes ${k}`), `missing @keyframes ${k}`).toBe(true);
  });

  it("with the same durations and curves", () => {
    const used = animationsOf(s.mock).filter((a) => !(s.skip as readonly string[]).some((k) => a.startsWith(`${k} `)));
    expect(used.length).toBeGreaterThan(0);
    for (const a of used) {
      // `fl` animates up to a variable (the measured width) and not to 94 %: the time and the curve are the same.
      has(timing(a).replace(/ forwards$/, "").replace(/ both$/, ""), a);
    }
  });

  it("and the transitions", () => {
    const trans = [...s.mock.matchAll(/transition:([^;}"]*)/g)].map((m) => canon(m[1])).filter((v) => !v.includes("none"));
    // Those of the shared components (button, card, switch) are tested in their own tests.
    const own = trans.filter((t) => /cubic-bezier\(0\.[23], 1\.?\d*/.test(t) || /^(background|transform) 0\.[567]s/.test(t));
    for (const t of own) has(t.split(",")[0], t);
  });
});

describe("Tunnels animations that the mockup declares inline", () => {
  it("the dot pulse lasts the same as in the mockup", () => {
    expect(SCREENS.tunnels.mock).toContain("animation:pl 1.8s infinite");
    for (const f of SCREENS.tunnels.files) expect(f).toContain("1.8s infinite");
  });

  it("the tube stops and turns off without motion when the tunnel is closed", () => {
    expect(SCREENS.tunnels.mock).toContain("animation:none;opacity:.2");
    expect(SCREENS.tunnels.files[0]).toMatch(/\.flowA\.off,\s*\.flowB\.off \{\s*animation: none;\s*opacity: 0\.2;/);
  });
});

describe("reduced motion in Monitors, Scene, Progress and Tunnels", () => {
  it("the global rule turns off animations and transitions", () => {
    expect(RULE).toContain("animation: none !important");
    expect(RULE).toContain("transition: none !important");
  });

  it("no rule of these screens escapes the global rule", () => {
    setReducedMotion(true);
    expect(reducedMotion()).toBe(true);
    for (const s of Object.values(SCREENS)) {
      for (const src of s.files) {
        // Without `!important` on animation/transition (it would beat the global rule) or animations from JS.
        expect(src).not.toMatch(/(animation|transition)[^;{}]*!important/);
        expect(src).not.toMatch(/\.animate\(|svelte\/transition|svelte\/animate/);
      }
    }
    setReducedMotion(false);
  });

  it("the measurement bar looks full without animation (the mockup does the same)", () => {
    const src = SCREENS.progress.files[0];
    expect(SCREENS.progress.mock).toContain(".meter span{width:94%}");
    expect(src).toMatch(/@media \(prefers-reduced-motion: reduce\) \{\s*\.meter span \{\s*width: var\(--w\);/);
  });
});
