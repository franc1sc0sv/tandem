import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { enter, leave, reducedMotion } from "../lib/motion";
import { setReducedMotion } from "../test/setup";

// G7 y G8 para los avisos (`F-Avisos`), las tarjetas de estado (`V3-Estados`) y las pestañas
// de Ajustes (`F-Ajustes`). Mismo modelo que `animaciones.test.ts` (T-116).
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
  avisos: {
    mock: mockup("F-Avisos"),
    css: read("shell", "Toasts.svelte"),
    // `dr` (la salida) es la de `motion.leave`: se prueba aparte. `sl` y `life` viven en el aviso.
    keyframes: ["sl", "life"],
    skip: ["dr"],
  },
  estados: {
    mock: mockup("V3-Estados"),
    css: read("ui", "StateCard.svelte"),
    keyframes: ["up"],
    skip: [],
  },
  "pestañas de ajustes": {
    mock: mockup("F-Ajustes"),
    css: read("screens", "Ajustes.svelte"),
    keyframes: ["fd"],
    skip: ["sp"],
  },
} as const;

describe.each(Object.entries(SCREENS))("animaciones de %s", (_name, s) => {
  const css = canon(s.css);

  it("cada @keyframes del mockup tiene su equivalente", () => {
    const names = keyframesOf(s.mock).filter((k) => !(s.skip as readonly string[]).includes(k));
    expect(names.length).toBeGreaterThan(0);
    // `up` y `fd` son de la hoja global (las comparten varias pantallas).
    for (const k of s.keyframes) expect(`${s.css}\n${GLOBAL}`.includes(`@keyframes ${k}`), `falta @keyframes ${k}`).toBe(true);
    expect(s.keyframes.length).toBeGreaterThanOrEqual(names.length);
  });

  it("con las mismas duraciones y curvas", () => {
    const used = animationsOf(s.mock).filter((a) => !(s.skip as readonly string[]).some((k) => a.startsWith(`${k} `)));
    expect(used.length).toBeGreaterThan(0);
    const mine = [...css.matchAll(/animation: ([^;]*);/g)].map((m) => m[1].replace(/^[\w-]+ /, ""));
    const theirs = used.map((a) => a.replace(/^\w+ /, "").replace(/ (forwards|both)$/, ""));
    for (const m of mine) expect(theirs.some((t) => m.startsWith(t) || t.startsWith(m)), `«${m}» no está en el mockup`).toBe(true);
    expect(mine.length).toBeGreaterThan(0);
  });
});

describe("detalles de cada animación", () => {
  it("el aviso entra con `sl` desde la derecha y su barra de vida dura lo que el aviso (6 s)", () => {
    const t = read("shell", "Toasts.svelte");
    expect(t).toMatch(/@keyframes sl \{\s*from \{\s*transform: translateX\(40px\);\s*opacity: 0;/);
    expect(t).toMatch(/@keyframes life \{\s*from \{\s*width: 100%;\s*\}\s*to \{\s*width: 0;/);
    expect(t).toContain('style:animation-duration="{TOAST_MS}ms"');
    expect(read("lib", "store", "store.svelte.ts")).toMatch(/export const TOAST_MS = 6000;/);
  });

  it("el aviso sale con la transición `leave` de motion.ts", () => {
    expect(read("shell", "Toasts.svelte")).toContain("out:leave");
    expect(leave(document.createElement("div"))).toMatchObject({ duration: 150 });
  });

  it("las tarjetas de estado entran una tras otra (0.04 s por tarjeta)", () => {
    expect(read("ui", "StateCard.svelte")).toContain('style:animation-delay="{index * 0.04}s"');
    expect(read("screens", "Estados.svelte")).toContain("index={i}");
  });

  it("cambiar de pestaña vuelve a montar el panel para que repita el fundido `fd`", () => {
    const a = read("screens", "Ajustes.svelte");
    expect(a).toMatch(/\{#key tab\}\s*<div class="pane">/);
    expect(GLOBAL).toMatch(/@keyframes fd \{\s*from \{\s*opacity: 0;\s*transform: translateX\(6px\);/);
  });
});

describe("prefers-reduced-motion en avisos, estados y pestañas (G8)", () => {
  it("la regla global apaga animaciones y transiciones", () => {
    expect(RULE).toContain("animation: none !important");
    expect(RULE).toContain("transition: none !important");
  });

  it("ninguna regla de estas pantallas se escapa de la regla global ni anima desde JS", () => {
    for (const src of Object.values(SCREENS).map((s) => s.css)) {
      expect(src).not.toMatch(/(animation|transition)[^;{}]*!important/);
      expect(src).not.toMatch(/\.animate\(|svelte\/transition|svelte\/animate/);
    }
  });

  it("con movimiento reducido la salida del aviso y la entrada duran 0 ms", () => {
    const el = document.createElement("div");
    setReducedMotion(true);
    expect(reducedMotion()).toBe(true);
    expect(leave(el)).toEqual({ duration: 0 });
    expect(enter(el, { duration: 300 })).toEqual({ duration: 0 });
    setReducedMotion(false);
    expect(leave(el)).toMatchObject({ duration: 150 });
  });
});
