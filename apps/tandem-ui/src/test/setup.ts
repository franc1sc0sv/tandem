// Entorno de las pruebas de componentes (jsdom): matchers de DOM y un `matchMedia` que se
// puede cambiar para probar `prefers-reduced-motion`.
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/svelte";
import { afterEach } from "vitest";

let reduced = false;

/** Simula la preferencia del sistema de reducir el movimiento. */
export function setReducedMotion(on: boolean): void {
  reduced = on;
}

afterEach(() => {
  cleanup();
  reduced = false;
});

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => ({
    matches: query.includes("prefers-reduced-motion") ? reduced : false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  }),
});

// jsdom no trae la API de animaciones web que usan las transiciones de Svelte.
if (!Element.prototype.animate) {
  Element.prototype.animate = function () {
    const anim = {
      finished: Promise.resolve(),
      cancel() {},
      finish() {},
      onfinish: null as null | (() => void),
      currentTime: 0,
      playState: "finished",
    };
    queueMicrotask(() => anim.onfinish?.());
    return anim as unknown as Animation;
  };
}
