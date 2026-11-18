// Prueba de cruce (`F-PruebaCruce`): se pide llevar el cursor a la otra máquina por el borde
// del acomodo y se juzga con lo que informa el motor de entrada. Puro: el tiempo entra como
// número para probarlo sin relojes.

import type { Machine, Side } from "./model";

/** Lo que el motor de entrada informa cuando el cursor pasa de una máquina a la otra. */
export interface CrossSample {
  from: Machine;
  to: Machine;
  /** El borde de `from` por el que salió. */
  edge: Side;
  /** Dónde entró, a lo largo del borde, en coordenadas de la pantalla de llegada. */
  at: number;
  /** Largo de ese borde (alto si el borde es vertical, ancho si es horizontal); 0 si el motor no dijo dónde entró. */
  of: number;
  /** Lo que tardó el cruce, en ms; `null` si el motor no lo midió (no llegó muestra). */
  ms: number | null;
}

/** El cursor tiene 20 s para cruzar. */
export const CROSS_TIMEOUT_MS = 20_000;

export type CrossPhase =
  | { kind: "waiting"; since: number }
  | { kind: "passed"; sample: CrossSample }
  | { kind: "wrong-edge"; sample: CrossSample }
  | { kind: "timeout" }
  /** La fuente no informa cruces (el agente todavía no manda `CursorSample`). */
  | { kind: "unavailable"; reason: string };

export function start(now: number): CrossPhase {
  return { kind: "waiting", since: now };
}

/**
 * Lo que pasa con un cruce informado. Solo cuenta mientras se espera y si sale de `from`:
 * un cruce de vuelta (de la otra máquina hacia aquí) no dice nada del borde.
 */
export function onSample(phase: CrossPhase, from: Machine, expected: Side, s: CrossSample): CrossPhase {
  if (phase.kind !== "waiting" || s.from !== from) return phase;
  return s.edge === expected ? { kind: "passed", sample: s } : { kind: "wrong-edge", sample: s };
}

/** Pasado el tiempo sin cruzar, la prueba falla. */
export function onTick(phase: CrossPhase, now: number, limit = CROSS_TIMEOUT_MS): CrossPhase {
  if (phase.kind === "waiting" && now - phase.since >= limit) return { kind: "timeout" };
  return phase;
}

/** Segundos que quedan (para el anillo de cuenta atrás). */
export function remaining(phase: CrossPhase, now: number, limit = CROSS_TIMEOUT_MS): number {
  if (phase.kind !== "waiting") return 0;
  return Math.max(0, Math.ceil((limit - (now - phase.since)) / 1000));
}

/** «0.4 ms», «12 ms». */
export function formatMs(ms: number): string {
  return ms < 10 ? `${(Math.round(ms * 10) / 10).toFixed(1)} ms` : `${Math.round(ms)} ms`;
}
