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

// G7 y G8 para el asistente de 6 pasos (`F-Asis1`, `V3-Asistente`, `F-Asis3`…`F-Asis6`).
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

/** Parte una lista por las comas de primer nivel (las de `cubic-bezier(...)` no cuentan). */
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

/** Cada animación del mockup, sin su nombre y sin el modo de relleno (la app usa `both`, el mockup `forwards`). */
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

// Todo lo que dibuja el asistente: su CSS global, el marco, los seis pasos y el botón.
const FILES = [
  read("shell", "wizard", "wizard.css"),
  read("shell", "Wizard.svelte"),
  ...readdirSync(join(SRC, "shell", "wizard"))
    .filter((f) => f.endsWith(".svelte"))
    .map((f) => read("shell", "wizard", f)),
  read("ui", "Button.svelte"),
];
const CSS = canon(FILES.join("\n"));
const has = (text: string, what: string) => expect(CSS.includes(text), `falta «${what}»`).toBe(true);

interface Step {
  mock: string;
  /** Cada @keyframes del mockup y su equivalente en la app. */
  keyframes: Record<string, string[]>;
  /** Los que el mockup declara y no usa. */
  unused?: string[];
}

const STEPS: Record<string, Step> = {
  "1 · esta máquina": { mock: "F-Asis1", keyframes: { up: ["wz-up"], sp: ["wz-sp"] } },
  "2 · tu puesto": { mock: "V3-Asistente", keyframes: { fdi: ["wz-fdi"] } },
  "3 · permisos": { mock: "F-Asis3", keyframes: { pl: ["wz-pl"], pop: ["wz-pop"] }, unused: ["sp"] },
  "4 · teclado y mouse": { mock: "F-Asis4", keyframes: { up: ["wz-up"], fl: ["wz-fl"] } },
  "5 · acomodo y prueba": { mock: "F-Asis5", keyframes: { mv: ["wz-mvx", "wz-mvy"], gl: ["wz-gl"], pop: ["wz-popy"] } },
  "6 · listo": { mock: "F-Asis6", keyframes: { draw: ["wz-draw"], draw2: ["wz-draw"], up: ["wz-up"], glow: ["wz-glow"] } },
};

describe.each(Object.entries(STEPS))("animaciones del paso %s", (_name, s) => {
  const html = mockup(s.mock);

  it("cada @keyframes del mockup tiene su equivalente", () => {
    const names = keyframesOf(html).filter((k) => !(s.unused ?? []).includes(k));
    expect(names.length).toBeGreaterThan(0);
    for (const k of names) {
      expect(s.keyframes[k], `sin equivalente para @keyframes ${k}`).toBeDefined();
      for (const app of s.keyframes[k]!) expect(FILES.join("\n").includes(`@keyframes ${app}`), `falta @keyframes ${app}`).toBe(true);
    }
  });

  it("con las mismas duraciones, retrasos y curvas", () => {
    const used = animationsOf(html);
    expect(used.length).toBeGreaterThan(0);
    for (const a of used) has(a, a);
  });

  it("y las mismas transiciones", () => {
    const used = transitionsOf(html);
    expect(used.length).toBeGreaterThan(0);
    for (const t of used) has(t, t);
  });
});

// ---- Lo que cada pantalla anima de verdad ------------------------------------------------------

async function mount(query: string, step: number) {
  const source = new DemoSource(demoOptions(query));
  const store = new Store(source, { hash: "#/puesto" });
  await store.start();
  store.openWizard(step);
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  const dlg = screen.getByRole("dialog", { name: "Asistente" });
  return { ...r, store, source, dlg };
}

/** Los retrasos de animación en línea, en segundos y en orden de aparición. */
const delays = (root: Element, selector: string) => [...root.querySelectorAll<HTMLElement>(selector)].map((e) => parseFloat(e.style.animationDelay));
/** Los de las filas del mockup (`class="chk" style="animation-delay:.1s"`). */
const mockDelays = (html: string, cls: string) => [...html.matchAll(new RegExp(`class="${cls}"[^>]*?animation-delay:([\\d.]+)s`, "g"))].map((m) => parseFloat(m[1]!));

describe("cada pantalla anima lo que anima su mockup", () => {
  it("paso 1: las filas de «Lo que encontró» entran escalonadas y la del servicio gira", async () => {
    const { dlg } = await mount("demo&still&admin=pending", 0);
    expect(delays(dlg, ".wz-chk")).toEqual(mockDelays(mockup("F-Asis1"), "chk"));
    expect(dlg.querySelectorAll(".wz-spin")).toHaveLength(1);
  });

  it("paso 2: el panel de los dígitos entra con fundido", async () => {
    const { dlg } = await mount("demo&still&peer=none&pair=code", 1);
    expect(mockup("V3-Asistente")).toContain("animation:fdi .35s ease-out");
    expect(dlg.querySelectorAll(".wz-panel").length).toBeGreaterThan(0);
  });

  it("paso 3: el punto de lo que falta late y el de lo concedido aparece con rebote", async () => {
    const { dlg } = await mount("demo&still&perms=missing", 2);
    expect(dlg.querySelectorAll(".wz-dot.wait")).toHaveLength(2);
    expect(dlg.querySelectorAll(".wz-perm.granted .wz-tick")).toHaveLength(1);
    expect(dlg.querySelectorAll(".wz-perm")).toHaveLength(3);
  });

  it("paso 4: las máquinas y sus dispositivos entran escalonados y el flujo corre", async () => {
    const { dlg } = await mount("demo&still", 3);
    expect(delays(dlg, ".wz-dev")).toEqual(mockDelays(mockup("F-Asis4"), "dev"));
    expect(delays(dlg, ".wz-mc")).toEqual([0, 0.1]);
    expect(dlg.querySelectorAll(".wz-flow")).toHaveLength(2);
  });

  it("paso 5: el cursor recorre el borde mientras espera y el borde brilla al cruzar", async () => {
    const { dlg, store } = await mount("demo&still&cross=ok", 4);
    expect(dlg.querySelector(".wz-cur.go")).toBeNull();
    screen.getByRole("button", { name: "Probar cruce" }).click();
    await tick();
    await waitFor(() => expect(store.wizardCrossed).toBe(true));
    expect(dlg.querySelector(".wz-edge.glow")).not.toBeNull();
  });

  it("paso 6: el círculo y la marca se dibujan, las filas entran escalonadas y el botón al final", async () => {
    const { dlg, store } = await mount("demo&still", 5);
    store.wizardCrossed = true;
    await tick();
    expect(dlg.querySelector(".wz-ring")).not.toBeNull();
    expect(dlg.querySelector(".wz-tickd")).not.toBeNull();
    expect(delays(dlg, ".wz-sum")).toEqual(mockDelays(mockup("F-Asis6"), "row").slice(0, 5));
    expect(dlg.querySelector(".wz-fin")).not.toBeNull();
    expect(mockup("F-Asis6")).toContain("animation:up .45s 1.6s both,glow 2.6s 2.2s ease-in-out infinite");
  });
});

describe("el botón anima también la opacidad en F-Asis3 y F-Asis5 (ACCEPTANCE G7)", () => {
  const withOpacity = (name: string) => /\.btn\{display[^}]*opacity \.3s/.test(mockup(name));

  it("solo esos dos mockups lo hacen, y solo esos dos pasos llevan la regla", async () => {
    const expected: Record<string, boolean> = { "F-Asis1": false, "V3-Asistente": false, "F-Asis3": true, "F-Asis4": false, "F-Asis5": true, "F-Asis6": false };
    for (const [name, yes] of Object.entries(expected)) expect(withOpacity(name), name).toBe(yes);
    const names = ["F-Asis1", "V3-Asistente", "F-Asis3", "F-Asis4", "F-Asis5", "F-Asis6"];
    for (const [step, name] of names.entries()) {
      const { dlg, unmount } = await mount("demo&still", step);
      expect(dlg.querySelector(".body")?.classList.contains("tight"), `paso ${step + 1}`).toBe(expected[name]);
      unmount();
    }
  });

  it("la regla del CSS agrega `opacity 0.3s` a todos los botones del paso", () => {
    const rule = /\.wiz \.body\.tight \.btn \{([^}]*)\}/.exec(read("shell", "wizard", "wizard.css"))?.[1] ?? "";
    expect(canon(rule)).toContain("opacity 0.3s");
    // Y «Continuar» la conserva mientras falta algo (.45).
    expect(canon(read("shell", "wizard", "wizard.css"))).toMatch(/\.wz-next \{ transition: opacity 0\.3s,/);
  });
});

// ---- Movimiento reducido (G8) ------------------------------------------------------------------

const GLOBAL = read("styles", "global.css");
const RULE = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(GLOBAL)?.[1] ?? "";
/** El CSS del asistente sin los cuerpos de los @keyframes (que sí arrancan en opacity 0). */
const WIZ_CSS = read("shell", "wizard", "wizard.css").replace(/@keyframes [\w-]+ \{(?:[^{}]|\{[^{}]*\})*\}/g, "");

describe("movimiento reducido en el asistente", () => {
  it("la regla global apaga animaciones y transiciones", () => {
    expect(RULE).toContain("animation: none !important");
    expect(RULE).toContain("transition: none !important");
  });

  it("ninguna regla del asistente se escapa de la regla global", () => {
    for (const src of FILES) {
      expect(src).not.toMatch(/(animation|transition)[^;{}]*!important/);
      expect(src).not.toMatch(/\.animate\(|svelte\/transition|svelte\/animate/);
    }
  });

  it("ningún elemento depende de su animación para verse", () => {
    // Sin la animación (`animation: none`) vale el estilo base: nada arranca en opacity 0.
    expect(WIZ_CSS).not.toMatch(/opacity: 0;/);
    for (const m of WIZ_CSS.matchAll(/animation:\s*([^;]*);/g)) {
      for (const p of topLevel(m[1]!.replace(/\s+/g, " "))) {
        // Las de entrada rellenan hacia adelante; las infinitas no rellenan.
        if (/^wz-(up|draw)\b/.test(p)) expect(p, p).toMatch(/\b(both|forwards)\b/);
        if (p.includes("infinite")) expect(p, p).not.toMatch(/\b(both|forwards)\b/);
      }
    }
  });

  it("el círculo y la marca del paso 6 se ven completos sin animación, como en el mockup", () => {
    expect(mockup("F-Asis6")).toMatch(/prefers-reduced-motion:reduce\)\{\*\{[^}]*stroke-dashoffset:0!important/);
    const block = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(read("shell", "wizard", "wizard.css"))?.[1] ?? "";
    expect(canon(block)).toContain(".wiz .wz-ring, .wiz .wz-tickd { stroke-dashoffset: 0;");
  });

  it("las transiciones de Svelte del marco duran 0 ms", async () => {
    setReducedMotion(true);
    expect(reducedMotion()).toBe(true);
    expect(enter(document.body, { y: 6, duration: 350 }).duration).toBe(0);
    expect(leave(document.body).duration).toBe(0);
    setReducedMotion(false);
    expect(enter(document.body, { y: 6, duration: 350 }).duration).toBe(350);
  });

  it.each([0, 1, 2, 3, 4, 5])("el paso %i muestra todo su contenido sin movimiento", async (step) => {
    setReducedMotion(true);
    const { dlg } = await mount("demo&still&perms=missing", step);
    expect(dlg.querySelector("h1")?.textContent?.length).toBeGreaterThan(0);
    // Sin animación, nada queda con estilos en línea que lo escondan.
    for (const el of dlg.querySelectorAll<HTMLElement>("[style]")) {
      expect(el.style.opacity, el.outerHTML.slice(0, 80)).not.toBe("0");
      expect(el.style.visibility).not.toBe("hidden");
    }
    setReducedMotion(false);
  });
});
