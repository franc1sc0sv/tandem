// Transiciones de Svelte que respetan `prefers-reduced-motion` (G8). Las animaciones de CSS
// ya las apaga la regla global de `global.css`; las de Svelte corren por JS y miran esto.

import { cubicOut } from "svelte/easing";
import type { TransitionConfig } from "svelte/transition";

export function reducedMotion(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export interface EnterParams {
  /** Desplazamiento vertical initial, en px. */
  y?: number;
  /** Desplazamiento horizontal initial, en px. */
  x?: number;
  scale?: number;
  duration?: number;
  delay?: number;
}

/** Entrada con fundido y desplazamiento (la `up`/`fd` de los mockups). Sin movimiento: 0 ms. */
export function enter(_node: Element, { y = 0, x = 0, scale = 1, duration = 250, delay = 0 }: EnterParams = {}): TransitionConfig {
  if (reducedMotion()) return { duration: 0 };
  return {
    duration,
    delay,
    easing: cubicOut,
    css: (t, u) => `opacity:${t};transform:translate(${u * x}px,${u * y}px) scale(${1 - (1 - scale) * u})`,
  };
}

/** Salida: solo fundido. */
export function leave(_node: Element, { duration = 150 }: { duration?: number } = {}): TransitionConfig {
  if (reducedMotion()) return { duration: 0 };
  return { duration, css: (t) => `opacity:${t}` };
}
