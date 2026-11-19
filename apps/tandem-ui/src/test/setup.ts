// Environment of the component tests (jsdom): DOM matchers and a `matchMedia` that can be
// changed to test `prefers-reduced-motion`.
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/svelte";
import { afterEach } from "vitest";

let reduced = false;

/** Simulates the system preference to reduce motion. */
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

// jsdom does not have the web animations API that Svelte transitions use.
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
