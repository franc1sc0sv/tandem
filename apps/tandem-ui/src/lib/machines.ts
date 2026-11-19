// The two sides of the desk as the UI sees them: names, colors and the progress of a
// transaction. Pure, to test without a DOM. Nothing assumes which system is on each side (D2).

import type { Caps, Desk, Link, Machine, MachineCaps, Step, TxnView } from "./model";

export const MACHINES: Machine[] = ["a", "b"];

export const LINK_NAME: Record<Link, string> = { usb4: "USB-C", ethernet: "Ethernet", wifi: "Wi-Fi" };

/** Colors assigned when pairing (D17) if the announcement does not carry one. */
export const DEFAULT_COLOR: Record<Machine, string> = { a: "#7aa7e6", b: "#a99be8" };

export function other(m: Machine): Machine {
  return m === "a" ? "b" : "a";
}

/** What `who` announced, if known. */
export function capsOf(caps: Caps, who: Machine): MachineCaps | null {
  if (caps.here.machine === who) return caps.here;
  return caps.peer && caps.peer.machine === who ? caps.peer : null;
}

/** Name of `who`: its announcement, what is saved in the desk or "machine A/B". */
export function machineName(who: Machine, caps?: Caps | null, desk?: Desk | null): string {
  const fromCaps = caps ? capsOf(caps, who)?.name : undefined;
  const fromDesk = desk?.machines[who]?.name;
  return fromCaps || fromDesk || `machine ${who.toUpperCase()}`;
}

export function machineColor(who: Machine, caps?: Caps | null, desk?: Desk | null): string {
  const fromCaps = caps ? capsOf(caps, who)?.color : undefined;
  return fromCaps || desk?.machines[who]?.color || DEFAULT_COLOR[who];
}

// ---------------------------------------------------------------------------------------
// Progress of a transaction (scene, power on or off).

export type StepStatus = "pending" | "run" | "done" | "failed" | "undone";

export type Outcome = { kind: "running" } | { kind: "committed" } | { kind: "failed"; error: string };

export interface Progress {
  txn: number;
  steps: Step[];
  status: StepStatus[];
  outcome: Outcome;
  /** The error of the step that failed, until the outcome arrives. */
  error?: string;
}

/** Short text of a step, with the machine name. */
export function stepLabel(s: Step, name: (m: Machine) => string): string {
  switch (s.step) {
    case "power_on":
      return `Power on ${name(s.machine)}`;
    case "power_off":
      return `Power off ${name(s.machine)}`;
    case "switch_monitor":
      return `Monitor ${s.monitor} → ${s.input === "hdmi" ? "HDMI" : "DP"}`;
    case "show_machine":
      return `Monitor ${s.monitor} → ${name(s.machine)}`;
    case "arrange":
      return "Arrange the desktops";
    case "geometry":
      return "Keyboard and mouse: new geometry";
  }
}

/** Advances the progress with a notice. Notices of another transaction are ignored. */
export function advance(p: Progress | null, e: TxnView): Progress | null {
  if (e.type === "started") {
    return { txn: e.txn, steps: e.steps, status: e.steps.map(() => "pending"), outcome: { kind: "running" } };
  }
  if (!p || p.txn !== e.txn) return p;
  const status = [...p.status];
  switch (e.type) {
    case "step_started":
      status[e.index] = "run";
      return { ...p, status };
    case "step_done":
      status[e.index] = "done";
      return { ...p, status };
    case "step_failed":
      status[e.index] = "failed";
      return { ...p, status, error: e.error };
    case "step_undone":
      status[e.index] = "undone";
      return { ...p, status };
    case "committed":
      return { ...p, outcome: { kind: "committed" } };
    case "failed":
      return { ...p, outcome: { kind: "failed", error: p.error ?? "a step failed" } };
  }
}

export function isRunning(p: Progress | null): boolean {
  return p !== null && p.outcome.kind === "running";
}
