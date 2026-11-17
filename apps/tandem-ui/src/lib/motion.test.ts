import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { setReducedMotion } from "../test/setup";
import { enter, leave, reducedMotion } from "./motion";

// vitest corre desde apps/tandem-ui.
const SRC = join(process.cwd(), "src");

function files(dir: string, ext: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? files(p, ext) : p.endsWith(ext) ? [p] : [];
  });
}

describe("prefers-reduced-motion (G8)", () => {
  const el = document.createElement("div");

  it("las transiciones de Svelte duran 0 ms si el sistema pide menos movimiento", () => {
    expect(reducedMotion()).toBe(false);
    expect(enter(el, { y: 8, duration: 280 }).duration).toBe(280);
    expect(leave(el).duration).toBe(150);
    setReducedMotion(true);
    expect(reducedMotion()).toBe(true);
    expect(enter(el, { y: 8, duration: 280 })).toEqual({ duration: 0 });
    expect(leave(el)).toEqual({ duration: 0 });
  });

  it("la entrada desplaza y funde (curva de los mockups)", () => {
    const css = enter(el, { y: 10, duration: 450 }).css;
    expect(css?.(0, 1)).toBe("opacity:0;transform:translate(0px,10px) scale(1)");
    expect(css?.(1, 0)).toBe("opacity:1;transform:translate(0px,0px) scale(1)");
  });

  it("la hoja global apaga toda animación y transición de CSS", () => {
    const css = readFileSync(join(SRC, "styles", "global.css"), "utf8");
    const rule = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(rule).toContain("*::before");
    expect(rule).toContain("animation: none !important");
    expect(rule).toContain("transition: none !important");
  });

  it("ningún componente anima sin pasar por esa regla (nada de animaciones por JS fuera de motion.ts)", () => {
    for (const f of files(SRC, ".svelte")) {
      const s = readFileSync(f, "utf8");
      expect(s, f).not.toMatch(/\.animate\(/);
      expect(s, f).not.toMatch(/from "svelte\/transition"/);
    }
  });
});
