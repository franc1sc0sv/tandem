// Crossing test (`F-CrossingTest`): the user is asked to take the cursor to the other machine through the
// arrangement edge and it is judged with what the input engine reports. Pure: time enters as a
// number to test without clocks.

import type { Machine, Side } from "./model";

/** What the input engine reports when the cursor goes from one machine to the other. */
export interface CrossSample {
  from: Machine;
  to: Machine;
  /** The edge of `from` it left through. */
  edge: Side;
  /** Where it entered, along the edge, in the coordinates of the arrival screen. */
  at: number;
  /** Length of that edge (height if the edge is vertical, width if horizontal); 0 if the engine did not say where it entered. */
  of: number;
  /** How long the crossing took, in ms; `null` if the engine did not measure it (no sample arrived). */
  ms: number | null;
}

/** The cursor has 20 s to cross. */
export const CROSS_TIMEOUT_MS = 20_000;

export type CrossPhase =
  | { kind: "waiting"; since: number }
  | { kind: "passed"; sample: CrossSample }
  | { kind: "wrong-edge"; sample: CrossSample }
  | { kind: "timeout" }
  /** The source does not report crossings (the agent does not send `CursorSample` yet). */
  | { kind: "unavailable"; reason: string };

export function start(now: number): CrossPhase {
  return { kind: "waiting", since: now };
}

/**
 * What happens with a reported crossing. It only counts while waiting and if it leaves `from`:
 * a crossing back (from the other machine to here) says nothing about the edge.
 */
export function onSample(phase: CrossPhase, from: Machine, expected: Side, s: CrossSample): CrossPhase {
  if (phase.kind !== "waiting" || s.from !== from) return phase;
  return s.edge === expected ? { kind: "passed", sample: s } : { kind: "wrong-edge", sample: s };
}

/** Once the time has passed without crossing, the test fails. */
export function onTick(phase: CrossPhase, now: number, limit = CROSS_TIMEOUT_MS): CrossPhase {
  if (phase.kind === "waiting" && now - phase.since >= limit) return { kind: "timeout" };
  return phase;
}

/** Seconds left (for the countdown ring). */
export function remaining(phase: CrossPhase, now: number, limit = CROSS_TIMEOUT_MS): number {
  if (phase.kind !== "waiting") return 0;
  return Math.max(0, Math.ceil((limit - (now - phase.since)) / 1000));
}

/** «0.4 ms», «12 ms». */
export function formatMs(ms: number): string {
  return ms < 10 ? `${(Math.round(ms * 10) / 10).toFixed(1)} ms` : `${Math.round(ms)} ms`;
}
