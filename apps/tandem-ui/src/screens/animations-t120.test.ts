import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { enter, leave, reducedMotion } from "../lib/motion";
import { setReducedMotion } from "../test/setup";

// G7 and G8 for the notices (`F-Notices`), the state cards (`V3-States`) and the
// Settings tabs (`F-Settings`). Same model as `animations.test.ts` (T-116).
const SRC = join(process.cwd(), "src");
const read = (...p: string[]) => readFileSync(join(SRC, ...p), "utf8");
const mockup = (name: string) => readFileSync(join(process.cwd(), "..", "..", "mockups", "v03", `${name}.dc.html`), "utf8");

const canon = (css: string) =>
  css
    .replace(/(^|[^\d])\.(\d)/g, "$10.$2")
    .replace(/,\s*/g, ", ")
    .replace(/\s+/g, " ")
    .trim();
const animationsOf = (html: string) => [...html.matchAll(/animation:([^;}"]*)/g)].map((m) => m[1]).filter((v) => !v.includes("none")).map(canon);
const keyframesOf = (html: string) => [...html.matchAll(/@keyframes (\w+)/g)].map((m) => m[1]).sort();

const GLOBAL = read("styles", "global.css");
const RULE = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(GLOBAL)?.[1] ?? "";

const SCREENS = {
  notices: {
    mock: mockup("F-Notices"),
    css: read("shell", "Toasts.svelte"),
    // `dr` (the exit) is that of `motion.leave`: tested separately. `sl` and `life` live in the notice.
    keyframes: ["sl", "life"],
    skip: ["dr"],
  },
  states: {
    mock: mockup("V3-States"),
    css: read("ui", "StateCard.svelte"),
    keyframes: ["up"],
    skip: [],
  },
  "settings tabs": {
    mock: mockup("F-Settings"),
    css: read("screens", "Settings.svelte"),
    keyframes: ["fd"],
    skip: ["sp"],
  },
} as const;

describe.each(Object.entries(SCREENS))("animations of %s", (_name, s) => {
  const css = canon(s.css);

  it("each mockup @keyframes has its equivalent", () => {
    const names = keyframesOf(s.mock).filter((k) => !(s.skip as readonly string[]).includes(k));
    expect(names.length).toBeGreaterThan(0);
    // `up` and `fd` belong to the global sheet (several screens share them).
    for (const k of s.keyframes) expect(`${s.css}\n${GLOBAL}`.includes(`@keyframes ${k}`), `missing @keyframes ${k}`).toBe(true);
    expect(s.keyframes.length).toBeGreaterThanOrEqual(names.length);
  });

  it("with the same durations and curves", () => {
    const used = animationsOf(s.mock).filter((a) => !(s.skip as readonly string[]).some((k) => a.startsWith(`${k} `)));
    expect(used.length).toBeGreaterThan(0);
    const mine = [...css.matchAll(/animation: ([^;]*);/g)].map((m) => m[1].replace(/^[\w-]+ /, ""));
    const theirs = used.map((a) => a.replace(/^\w+ /, "").replace(/ (forwards|both)$/, ""));
    for (const m of mine) expect(theirs.some((t) => m.startsWith(t) || t.startsWith(m)), `"${m}" is not in the mockup`).toBe(true);
    expect(mine.length).toBeGreaterThan(0);
  });
});

describe("details of each animation", () => {
  it("the notice enters with `sl` from the right and its life bar lasts as long as the notice (6 s)", () => {
    const t = read("shell", "Toasts.svelte");
    expect(t).toMatch(/@keyframes sl \{\s*from \{\s*transform: translateX\(40px\);\s*opacity: 0;/);
    expect(t).toMatch(/@keyframes life \{\s*from \{\s*width: 100%;\s*\}\s*to \{\s*width: 0;/);
    expect(t).toContain('style:animation-duration="{TOAST_MS}ms"');
    expect(read("lib", "store", "store.svelte.ts")).toMatch(/export const TOAST_MS = 6000;/);
  });

  it("the notice leaves with the `leave` transition of motion.ts", () => {
    expect(read("shell", "Toasts.svelte")).toContain("out:leave");
    expect(leave(document.createElement("div"))).toMatchObject({ duration: 150 });
  });

  it("the state cards enter one after another (0.04 s per card)", () => {
    expect(read("ui", "StateCard.svelte")).toContain('style:animation-delay="{index * 0.04}s"');
    expect(read("screens", "States.svelte")).toContain("index={i}");
  });

  it("switching tabs remounts the panel so it repeats the `fd` fade", () => {
    const a = read("screens", "Settings.svelte");
    expect(a).toMatch(/\{#key tab\}\s*<div class="pane">/);
    expect(GLOBAL).toMatch(/@keyframes fd \{\s*from \{\s*opacity: 0;\s*transform: translateX\(6px\);/);
  });
});

describe("prefers-reduced-motion in notices, states and tabs (G8)", () => {
  it("the global rule turns off animations and transitions", () => {
    expect(RULE).toContain("animation: none !important");
    expect(RULE).toContain("transition: none !important");
  });

  it("no rule of these screens escapes the global rule or animates from JS", () => {
    for (const src of Object.values(SCREENS).map((s) => s.css)) {
      expect(src).not.toMatch(/(animation|transition)[^;{}]*!important/);
      expect(src).not.toMatch(/\.animate\(|svelte\/transition|svelte\/animate/);
    }
  });

  it("with reduced motion the notice exit and entrance last 0 ms", () => {
    const el = document.createElement("div");
    setReducedMotion(true);
    expect(reducedMotion()).toBe(true);
    expect(leave(el)).toEqual({ duration: 0 });
    expect(enter(el, { duration: 300 })).toEqual({ duration: 0 });
    setReducedMotion(false);
    expect(leave(el)).toMatchObject({ duration: 150 });
  });
});
