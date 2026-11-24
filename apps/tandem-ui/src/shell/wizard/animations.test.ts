import { render, screen, waitFor } from "@testing-library/svelte";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { tick } from "svelte";
import { describe, expect, it } from "vitest";

import { STORE_KEY } from "../../lib/context";
import { enter, leave, reducedMotion } from "../../lib/motion";
import { DemoSource, demoOptions } from "../../lib/store/demo";
import { Store } from "../../lib/store/store.svelte";
import { setReducedMotion } from "../../test/setup";
import Shell from "../Shell.svelte";

// G7 and G8 for the 6-step setup (`F-Setup1`, `V3-Setup`, `F-Setup3`…`F-Setup6`).
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

/** Splits a list on top-level commas (those of `cubic-bezier(...)` do not count). */
function topLevel(list: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let cur = "";
  for (const c of list) {
    if (c === "(") depth++;
    if (c === ")") depth--;
    if (c === "," && depth === 0) {
      out.push(cur);
      cur = "";
    } else cur += c;
  }
  out.push(cur);
  return out.map((p) => p.trim()).filter(Boolean);
}

/** Each mockup animation, without its name and without the fill mode (the app uses `both`, the mockup `forwards`). */
const animationsOf = (html: string) =>
  [...html.matchAll(/animation:([^;}"]*)/g)]
    .map((m) => m[1]!)
    .filter((v) => !v.includes("none"))
    .flatMap(topLevel)
    .map(canon)
    .map((a) => a.replace(/^\w+ /, "").replace(/ (forwards|both)$/, ""));

const transitionsOf = (html: string) =>
  [...html.matchAll(/transition:([^;}"]*)/g)]
    .map((m) => m[1]!)
    .filter((v) => !v.includes("none"))
    .flatMap(topLevel)
    .map(canon);

const keyframesOf = (html: string) => [...html.matchAll(/@keyframes (\w+)/g)].map((m) => m[1]!);

// Everything the setup draws: its global CSS, the frame, the six steps and the button.
const FILES = [
  read("shell", "wizard", "wizard.css"),
  read("shell", "Wizard.svelte"),
  ...readdirSync(join(SRC, "shell", "wizard"))
    .filter((f) => f.endsWith(".svelte"))
    .map((f) => read("shell", "wizard", f)),
  read("ui", "Button.svelte"),
];
const CSS = canon(FILES.join("\n"));
const has = (text: string, what: string) => expect(CSS.includes(text), `missing "${what}"`).toBe(true);

interface Step {
  mock: string;
  /** Each mockup @keyframes and its equivalent in the app. */
  keyframes: Record<string, string[]>;
  /** Those the mockup declares and does not use. */
  unused?: string[];
}

const STEPS: Record<string, Step> = {
  "1 · this machine": { mock: "F-Setup1", keyframes: { up: ["wz-up"], sp: ["wz-sp"] } },
  "2 · tu desk": { mock: "V3-Setup", keyframes: { fdi: ["wz-fdi"] } },
  "3 · permissions": { mock: "F-Setup3", keyframes: { pl: ["wz-pl"], pop: ["wz-pop"] }, unused: ["sp"] },
  "4 · keyboard and mouse": { mock: "F-Setup4", keyframes: { up: ["wz-up"], fl: ["wz-fl"] } },
  "5 · arrangement and test": { mock: "F-Setup5", keyframes: { mv: ["wz-mvx", "wz-mvy"], gl: ["wz-gl"], pop: ["wz-popy"] } },
  "6 · done": { mock: "F-Setup6", keyframes: { draw: ["wz-draw"], draw2: ["wz-draw"], up: ["wz-up"], glow: ["wz-glow"] } },
};

describe.each(Object.entries(STEPS))("animations of step %s", (_name, s) => {
  const html = mockup(s.mock);

  it("each mockup @keyframes has its equivalent", () => {
    const names = keyframesOf(html).filter((k) => !(s.unused ?? []).includes(k));
    expect(names.length).toBeGreaterThan(0);
    for (const k of names) {
      expect(s.keyframes[k], `no equivalent for @keyframes ${k}`).toBeDefined();
      for (const app of s.keyframes[k]!) expect(FILES.join("\n").includes(`@keyframes ${app}`), `missing @keyframes ${app}`).toBe(true);
    }
  });

  it("with the same durations, delays and curves", () => {
    const used = animationsOf(html);
    expect(used.length).toBeGreaterThan(0);
    for (const a of used) has(a, a);
  });

  it("and the same transitions", () => {
    const used = transitionsOf(html);
    expect(used.length).toBeGreaterThan(0);
    for (const t of used) has(t, t);
  });
});

// ---- What each screen really animates ----------------------------------------------------------

async function mount(query: string, step: number) {
  const source = new DemoSource(demoOptions(query));
  const store = new Store(source, { hash: "#/desk" });
  await store.start();
  store.openWizard(step);
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  const dlg = screen.getByRole("dialog", { name: "Setup" });
  return { ...r, store, source, dlg };
}

/** The inline animation delays, in seconds and in order of appearance. */
const delays = (root: Element, selector: string) => [...root.querySelectorAll<HTMLElement>(selector)].map((e) => parseFloat(e.style.animationDelay));
/** Those of the mockup rows (`class="chk" style="animation-delay:.1s"`). */
const mockDelays = (html: string, cls: string) => [...html.matchAll(new RegExp(`class="${cls}"[^>]*?animation-delay:([\\d.]+)s`, "g"))].map((m) => parseFloat(m[1]!));

describe("each screen animates what its mockup animates", () => {
  it("step 1: the \"What it found\" rows enter staggered and the service one spins", async () => {
    const { dlg } = await mount("demo&still&admin=pending", 0);
    expect(delays(dlg, ".wz-chk")).toEqual(mockDelays(mockup("F-Setup1"), "chk"));
    expect(dlg.querySelectorAll(".wz-spin")).toHaveLength(1);
  });

  it("step 2: the digits panel enters with a fade", async () => {
    const { dlg } = await mount("demo&still&peer=none&pair=code", 1);
    expect(mockup("V3-Setup")).toContain("animation:fdi .35s ease-out");
    expect(dlg.querySelectorAll(".wz-panel").length).toBeGreaterThan(0);
  });

  it("step 3: the missing dot pulses and the granted one appears with a bounce", async () => {
    const { dlg } = await mount("demo&still&perms=missing", 2);
    expect(dlg.querySelectorAll(".wz-dot.wait")).toHaveLength(2);
    expect(dlg.querySelectorAll(".wz-perm.granted .wz-tick")).toHaveLength(1);
    expect(dlg.querySelectorAll(".wz-perm")).toHaveLength(3);
  });

  it("step 4: the machines and their devices enter staggered and the flow runs", async () => {
    const { dlg } = await mount("demo&still", 3);
    expect(delays(dlg, ".wz-dev")).toEqual(mockDelays(mockup("F-Setup4"), "dev"));
    expect(delays(dlg, ".wz-mc")).toEqual([0, 0.1]);
    expect(dlg.querySelectorAll(".wz-flow")).toHaveLength(2);
  });

  it("step 5: the cursor travels the edge while waiting and the edge glows on crossing", async () => {
    const { dlg, store } = await mount("demo&still&cross=ok", 4);
    expect(dlg.querySelector(".wz-cur.go")).toBeNull();
    screen.getByRole("button", { name: "Test crossing" }).click();
    await tick();
    await waitFor(() => expect(store.wizardCrossed).toBe(true));
    expect(dlg.querySelector(".wz-edge.glow")).not.toBeNull();
  });

  it("step 6: the circle and the mark are drawn, the rows enter staggered and the button last", async () => {
    const { dlg, store } = await mount("demo&still", 5);
    store.wizardCrossed = true;
    await tick();
    expect(dlg.querySelector(".wz-ring")).not.toBeNull();
    expect(dlg.querySelector(".wz-tickd")).not.toBeNull();
    expect(delays(dlg, ".wz-sum")).toEqual(mockDelays(mockup("F-Setup6"), "row").slice(0, 5));
    expect(dlg.querySelector(".wz-fin")).not.toBeNull();
    expect(mockup("F-Setup6")).toContain("animation:up .45s 1.6s both,glow 2.6s 2.2s ease-in-out infinite");
  });
});

describe("the button also animates opacity in F-Setup3 and F-Setup5 (ACCEPTANCE G7)", () => {
  const withOpacity = (name: string) => /\.btn\{display[^}]*opacity \.3s/.test(mockup(name));

  it("only those two mockups do it, and only those two steps carry the rule", async () => {
    const expected: Record<string, boolean> = { "F-Setup1": false, "V3-Setup": false, "F-Setup3": true, "F-Setup4": false, "F-Setup5": true, "F-Setup6": false };
    for (const [name, yes] of Object.entries(expected)) expect(withOpacity(name), name).toBe(yes);
    const names = ["F-Setup1", "V3-Setup", "F-Setup3", "F-Setup4", "F-Setup5", "F-Setup6"];
    for (const [step, name] of names.entries()) {
      const { dlg, unmount } = await mount("demo&still", step);
      expect(dlg.querySelector(".body")?.classList.contains("tight"), `step ${step + 1}`).toBe(expected[name]);
      unmount();
    }
  });

  it("the CSS rule adds `opacity 0.3s` to all the step's buttons", () => {
    const rule = /\.wiz \.body\.tight \.btn \{([^}]*)\}/.exec(read("shell", "wizard", "wizard.css"))?.[1] ?? "";
    expect(canon(rule)).toContain("opacity 0.3s");
    // And "Continue" keeps it while something is missing (.45).
    expect(canon(read("shell", "wizard", "wizard.css"))).toMatch(/\.wz-next \{ transition: opacity 0\.3s,/);
  });
});

// ---- Reduced motion (G8) ----------------------------------------------------------------------

const GLOBAL = read("styles", "global.css");
const RULE = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(GLOBAL)?.[1] ?? "";
/** The setup CSS without the @keyframes bodies (which do start at opacity 0). */
const WIZ_CSS = read("shell", "wizard", "wizard.css").replace(/@keyframes [\w-]+ \{(?:[^{}]|\{[^{}]*\})*\}/g, "");

describe("reduced motion in the setup", () => {
  it("the global rule turns off animations and transitions", () => {
    expect(RULE).toContain("animation: none !important");
    expect(RULE).toContain("transition: none !important");
  });

  it("no setup rule escapes the global rule", () => {
    for (const src of FILES) {
      expect(src).not.toMatch(/(animation|transition)[^;{}]*!important/);
      expect(src).not.toMatch(/\.animate\(|svelte\/transition|svelte\/animate/);
    }
  });

  it("no element depends on its animation to be seen", () => {
    // Without the animation (`animation: none`) the base style holds: nothing starts at opacity 0.
    expect(WIZ_CSS).not.toMatch(/opacity: 0;/);
    for (const m of WIZ_CSS.matchAll(/animation:\s*([^;]*);/g)) {
      for (const p of topLevel(m[1]!.replace(/\s+/g, " "))) {
        // The entrance ones fill forwards; the infinite ones do not fill.
        if (/^wz-(up|draw)\b/.test(p)) expect(p, p).toMatch(/\b(both|forwards)\b/);
        if (p.includes("infinite")) expect(p, p).not.toMatch(/\b(both|forwards)\b/);
      }
    }
  });

  it("the circle and the mark of step 6 are seen whole without animation, like the mockup", () => {
    expect(mockup("F-Setup6")).toMatch(/prefers-reduced-motion:reduce\)\{\*\{[^}]*stroke-dashoffset:0!important/);
    const block = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(read("shell", "wizard", "wizard.css"))?.[1] ?? "";
    expect(canon(block)).toContain(".wiz .wz-ring, .wiz .wz-tickd { stroke-dashoffset: 0;");
  });

  it("the Svelte transitions of the frame last 0 ms", async () => {
    setReducedMotion(true);
    expect(reducedMotion()).toBe(true);
    expect(enter(document.body, { y: 6, duration: 350 }).duration).toBe(0);
    expect(leave(document.body).duration).toBe(0);
    setReducedMotion(false);
    expect(enter(document.body, { y: 6, duration: 350 }).duration).toBe(350);
  });

  it.each([0, 1, 2, 3, 4, 5])("step %i shows all its content without motion", async (step) => {
    setReducedMotion(true);
    const { dlg } = await mount("demo&still&perms=missing", step);
    expect(dlg.querySelector("h1")?.textContent?.length).toBeGreaterThan(0);
    // Without animation, nothing is left with inline styles that hide it.
    for (const el of dlg.querySelectorAll<HTMLElement>("[style]")) {
      expect(el.style.opacity, el.outerHTML.slice(0, 80)).not.toBe("0");
      expect(el.style.visibility).not.toBe("hidden");
    }
    setReducedMotion(false);
  });
});
