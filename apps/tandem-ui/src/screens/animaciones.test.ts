import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { setReducedMotion } from "../test/setup";
import { reducedMotion } from "../lib/motion";

// G7 y G8 para Monitores, Escena y Progreso (`V3-Monitores`, `F-NuevaEscena`, `F-Transacciones`).
// vitest corre desde apps/tandem-ui.
const SRC = join(process.cwd(), "src");
const read = (...p: string[]) => readFileSync(join(SRC, ...p), "utf8");
const mockup = (name: string) => readFileSync(join(process.cwd(), "..", "..", "mockups", "v03", `${name}.dc.html`), "utf8");

/** `.45s` -> `0.45s`, `cubic-bezier(.2,.8,.2,1)` -> `cubic-bezier(0.2, 0.8, 0.2, 1)`: como lo escribe el CSS de la app. */
const canon = (css: string) =>
  css
    .replace(/(^|[^\d])\.(\d)/g, "$10.$2")
    .replace(/,\s*/g, ", ")
    .replace(/\s+/g, " ")
    .trim();

/** Las declaraciones `animation:` del mockup (sin la del movimiento reducido) en la forma canónica. */
const animationsOf = (html: string) => [...html.matchAll(/animation:([^;}"]*)/g)].map((m) => m[1]).filter((v) => !v.includes("none")).map(canon);
const keyframesOf = (html: string) => [...html.matchAll(/@keyframes (\w+)/g)].map((m) => m[1]).sort();

/** El CSS de la regla global de `prefers-reduced-motion`. */
const GLOBAL = read("styles", "global.css");
const RULE = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(GLOBAL)?.[1] ?? "";

/** Duración y curva de cada animación del mockup (sin el nombre, que cambia con el prefijo de la pantalla). */
const timing = (decl: string) => decl.replace(/^\w+ /, "");

const SCREENS = {
  monitores: {
    mock: mockup("V3-Monitores"),
    // `up`, `in` y `pl` viven en la pantalla; `fls` (el destello de la miniatura) en `Thumb`.
    files: [read("screens", "Monitores.svelte"), read("ui", "Thumb.svelte")],
    keyframes: ["mon-up", "mon-in", "mon-pl", "thumb-fls"],
    skip: ["dr"],
  },
  escena: {
    mock: mockup("F-NuevaEscena"),
    files: [read("screens", "Escena.svelte")],
    keyframes: ["es-bd"],
    skip: [],
  },
  progreso: {
    mock: mockup("F-Transacciones"),
    files: [read("screens", "Progreso.svelte")],
    keyframes: ["pr-sp", "pr-wv", "pr-fl", "pr-up"],
    skip: ["cnt"],
  },
} as const;

describe.each(Object.entries(SCREENS))("animaciones de %s", (_name, s) => {
  // La pantalla y los componentes compartidos que dibujan parte del mockup (tarjeta, miniatura, selector, botón).
  const css = [...s.files, read("ui", "Card.svelte"), read("ui", "Thumb.svelte"), read("ui", "Segmented.svelte"), read("ui", "Button.svelte")].join("\n");
  const has = (text: string, what: string) => expect(canon(css).includes(text), `falta «${what}»`).toBe(true);

  it("cada @keyframes del mockup tiene su equivalente", () => {
    const names = keyframesOf(s.mock).filter((k) => !(s.skip as readonly string[]).includes(k));
    // Un equivalente por cada uno del mockup (menos los que el mockup declara y no usa).
    expect(s.keyframes.length).toBeGreaterThanOrEqual(names.length);
    for (const k of s.keyframes) expect(css.includes(`@keyframes ${k}`), `falta @keyframes ${k}`).toBe(true);
  });

  it("con las mismas duraciones y curvas", () => {
    const used = animationsOf(s.mock).filter((a) => !(s.skip as readonly string[]).some((k) => a.startsWith(`${k} `)));
    expect(used.length).toBeGreaterThan(0);
    for (const a of used) {
      // `fl` anima hasta una variable (el ancho medido) y no hasta 94 %: el tiempo y la curva son los mismos.
      has(timing(a).replace(/ forwards$/, "").replace(/ both$/, ""), a);
    }
  });

  it("y las transiciones", () => {
    const trans = [...s.mock.matchAll(/transition:([^;}"]*)/g)].map((m) => canon(m[1])).filter((v) => !v.includes("none"));
    // Las de los componentes compartidos (botón, tarjeta, interruptor) se prueban en sus propias pruebas.
    const own = trans.filter((t) => /cubic-bezier\(0\.[23], 1\.?\d*/.test(t) || /^(background|transform) 0\.[567]s/.test(t));
    for (const t of own) has(t.split(",")[0], t);
  });
});

describe("movimiento reducido en Monitores, Escena y Progreso", () => {
  it("la regla global apaga animaciones y transiciones", () => {
    expect(RULE).toContain("animation: none !important");
    expect(RULE).toContain("transition: none !important");
  });

  it("ninguna regla de estas pantallas se escapa de la regla global", () => {
    setReducedMotion(true);
    expect(reducedMotion()).toBe(true);
    for (const s of Object.values(SCREENS)) {
      for (const src of s.files) {
        // Sin `!important` sobre animation/transition (ganaría a la regla global) ni animaciones desde JS.
        expect(src).not.toMatch(/(animation|transition)[^;{}]*!important/);
        expect(src).not.toMatch(/\.animate\(|svelte\/transition|svelte\/animate/);
      }
    }
    setReducedMotion(false);
  });

  it("la barra de la medición se ve llena sin animación (el mockup lo hace igual)", () => {
    const src = SCREENS.progreso.files[0];
    expect(SCREENS.progreso.mock).toContain(".meter span{width:94%}");
    expect(src).toMatch(/@media \(prefers-reduced-motion: reduce\) \{\s*\.meter span \{\s*width: var\(--w\);/);
  });
});
