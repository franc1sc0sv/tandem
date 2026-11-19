import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { setReducedMotion } from "../test/setup";
import { enter, leave, reducedMotion } from "./motion";

// vitest runs from apps/tandem-ui.
const SRC = join(process.cwd(), "src");

function files(dir: string, ext: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? files(p, ext) : p.endsWith(ext) ? [p] : [];
  });
}

describe("prefers-reduced-motion (G8)", () => {
  const el = document.createElement("div");

  it("Svelte transitions last 0 ms if the system asks for less motion", () => {
    expect(reducedMotion()).toBe(false);
    expect(enter(el, { y: 8, duration: 280 }).duration).toBe(280);
    expect(leave(el).duration).toBe(150);
    setReducedMotion(true);
    expect(reducedMotion()).toBe(true);
    expect(enter(el, { y: 8, duration: 280 })).toEqual({ duration: 0 });
    expect(leave(el)).toEqual({ duration: 0 });
  });

  it("the entrance offsets and fades (curve of the mockups)", () => {
    const css = enter(el, { y: 10, duration: 450 }).css;
    expect(css?.(0, 1)).toBe("opacity:0;transform:translate(0px,10px) scale(1)");
    expect(css?.(1, 0)).toBe("opacity:1;transform:translate(0px,0px) scale(1)");
  });

  it("the global sheet turns off every CSS animation and transition", () => {
    const css = readFileSync(join(SRC, "styles", "global.css"), "utf8");
    const rule = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(rule).toContain("*::before");
    expect(rule).toContain("animation: none !important");
    expect(rule).toContain("transition: none !important");
  });

  it("no component animates without going through that rule: nothing animated by JS outside motion.ts", () => {
    for (const f of files(SRC, ".svelte")) {
      const s = readFileSync(f, "utf8");
      expect(s, f).not.toMatch(/\.animate\(/);
      expect(s, f).not.toMatch(/from "svelte\/transition"/);
    }
  });
});
