// Svelte transitions that respect `prefers-reduced-motion` (G8). CSS animations
// are already turned off by the global rule in `global.css`; Svelte's run in JS and look at this.

import { cubicOut } from "svelte/easing";
import type { TransitionConfig } from "svelte/transition";

export function reducedMotion(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export interface EnterParams {
  /** Initial vertical offset, in px. */
  y?: number;
  /** Initial horizontal offset, in px. */
  x?: number;
  scale?: number;
  duration?: number;
  delay?: number;
}

/** Entrance with fade and offset (the `up`/`fd` of the mockups). Without motion: 0 ms. */
export function enter(_node: Element, { y = 0, x = 0, scale = 1, duration = 250, delay = 0 }: EnterParams = {}): TransitionConfig {
  if (reducedMotion()) return { duration: 0 };
  return {
    duration,
    delay,
    easing: cubicOut,
    css: (t, u) => `opacity:${t};transform:translate(${u * x}px,${u * y}px) scale(${1 - (1 - scale) * u})`,
  };
}

/** Exit: fade only. */
export function leave(_node: Element, { duration = 150 }: { duration?: number } = {}): TransitionConfig {
  if (reducedMotion()) return { duration: 0 };
  return { duration, css: (t) => `opacity:${t}` };
}
