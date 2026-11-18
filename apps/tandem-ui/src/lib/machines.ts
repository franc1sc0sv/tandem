// Los dos lados del puesto vistos desde la UI: nombres, colores y el avance de una
// transacción. Puro, para probarlo sin DOM. Nada supone qué sistema hay en cada lado (D2).

import type { Caps, Desk, Link, Machine, MachineCaps, Step, TxnView } from "./model";

export const MACHINES: Machine[] = ["a", "b"];

export const LINK_NAME: Record<Link, string> = { usb4: "USB-C", ethernet: "Ethernet", wifi: "Wi-Fi" };

/** Colores que se asignan al emparejar (D17) si el anuncio no trae uno. */
export const DEFAULT_COLOR: Record<Machine, string> = { a: "#7aa7e6", b: "#a99be8" };

export function other(m: Machine): Machine {
  return m === "a" ? "b" : "a";
}

/** Lo que anunció `who`, si se sabe. */
export function capsOf(caps: Caps, who: Machine): MachineCaps | null {
  if (caps.here.machine === who) return caps.here;
  return caps.peer && caps.peer.machine === who ? caps.peer : null;
}

/** Nombre de `who`: su anuncio, lo guardado en el puesto o «máquina A/B». */
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
// Avance de una transacción (escena, encender o apagar).

export type StepStatus = "pending" | "run" | "done" | "failed" | "undone";

export type Outcome = { kind: "running" } | { kind: "committed" } | { kind: "failed"; error: string };

export interface Progress {
  txn: number;
  steps: Step[];
  status: StepStatus[];
  outcome: Outcome;
  /** El error del paso que falló, hasta que llegue el desenlace. */
  error?: string;
}

/** Texto corto de un paso, con el nombre de la máquina. */
export function stepLabel(s: Step, name: (m: Machine) => string): string {
  switch (s.step) {
    case "power_on":
      return `Encender ${name(s.machine)}`;
    case "power_off":
      return `Apagar ${name(s.machine)}`;
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

/** Avanza el progreso con un aviso. Los avisos de otra transacción se ignoran. */
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
