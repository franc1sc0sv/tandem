// The other two cards of "Changes in progress" (`F-Transactions`): powering on a
// machine and measuring a link. Pure: the screen only draws what comes out of here.

import { LINK_NAME } from "./machines";
import type { Machine } from "./model";
import { formatMs, formatRate, RING_GBPS, unpluggedText } from "./network";
import type { LinkKind, NetworkView, PathView } from "./store/types";
import type { Run } from "./txn";

/** How long to wait for the powered-on machine's agent to answer (DESIGN §3.6). */
export const BOOT_WAIT_MS = 120_000;
/** How often the Wake-on-LAN packet is resent. */
export const WOL_EVERY_S = 5;

export type StepIcon = "done" | "run" | "bad" | "todo";

export interface EnergyRow {
  key: string;
  icon: StepIcon;
  text: string;
  detail: string;
  /** The detail goes in monospace (the clock). */
  mono: boolean;
  /** Dimmed row (what has not started yet). */
  dim: boolean;
}

export interface EnergyView {
  title: string;
  rows: EnergyRow[];
  /** While waiting, it is possible to stop waiting. */
  waiting: boolean;
}

/** `0:18`. */
export function clockText(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** The machine being powered on in this run, if it is one. */
export function powerTarget(run: Run | null): Machine | null {
  const s = run?.progress.steps[0];
  return s?.step === "power_on" ? s.machine : null;
}

/** The power card of a `power_on` run; `null` if the run is something else. */
export function energyView(run: Run | null, name: (m: Machine) => string, from: string, now: number): EnergyView | null {
  const target = powerTarget(run);
  if (!run || !target) return null;
  const st = run.progress.status[0];
  const waited = clockText((run.endedAt ?? now) - run.startedAt);
  const rows: EnergyRow[] = [
    { key: "wol", icon: "done", text: `Wake-on-LAN sent from ${from}`, detail: st === "done" ? "done" : `every ${WOL_EVERY_S} s`, mono: false, dim: false },
    {
      key: "wait",
      icon: st === "done" ? "done" : st === "failed" ? "bad" : "run",
      text: "Waiting for its agent to answer",
      detail: st === "done" ? waited : st === "failed" ? (run.progress.error ?? "did not answer") : `${waited} / ${clockText(BOOT_WAIT_MS)}`,
      mono: st !== "failed",
      dim: false,
    },
    { key: "join", icon: st === "done" ? "done" : "todo", text: "Add it back to the desk", detail: "", mono: false, dim: st !== "done" },
  ];
  return { title: `Powering on ${name(target)}`, rows, waiting: st === "run" || st === "pending" };
}

export interface MeterRow {
  name: string;
  text: string;
}

export interface MeasureView {
  kind: LinkKind;
  title: string;
  /** The big number: "9.4" and its unit. */
  value: string;
  unit: string;
  latency: string;
  /** 0–100: what was measured against the bar's scale. */
  percent: number;
  /** `9.4 of 10 Gbps`, for screen readers. */
  aria: string;
  caption: string;
  others: MeterRow[];
}

const CAPTION: Record<LinkKind, string> = {
  usb4: "USB4 20 Gbps link",
  ethernet: "Ethernet 2.5 Gbps link",
  wifi: "Wi-Fi: negotiated link",
};

const otherText = (p: PathView): string => {
  if (p.state === "unplugged") return unpluggedText(p.kind);
  if (p.state === "down") return "down";
  return [p.gbps === null ? null : formatRate(p.gbps), p.rttMs === null ? null : formatMs(p.rttMs)].filter(Boolean).join(" · ") || "—";
};

/** The path being measured now, or the last one measured; `null` if there is no measurement to show. */
export function measureKind(n: NetworkView, last: LinkKind | null): LinkKind | null {
  return n.measuring ?? last;
}

export function measureView(n: NetworkView, kind: LinkKind): MeasureView | null {
  const path = n.paths.find((p) => p.kind === kind);
  if (!path) return null;
  const g = path.gbps;
  const [value, unit] = g === null ? ["—", "Gbps"] : g >= 1 ? [String(Math.round(g * 10) / 10), "Gbps"] : [String(Math.round(g * 1000)), "Mbps"];
  const percent = g === null ? 0 : Math.max(0, Math.min(100, Math.round((g / RING_GBPS) * 100)));
  return {
    kind,
    title: `Measuring the ${LINK_NAME[kind]} link`,
    value,
    unit,
    latency: path.rttMs === null ? "—" : formatMs(path.rttMs),
    percent,
    aria: `${g === null ? "not measured" : `${Math.round(g * 10) / 10}`} of ${RING_GBPS} Gbps`,
    caption: CAPTION[kind],
    others: n.paths.filter((p) => p.kind !== kind).map((p) => ({ name: LINK_NAME[p.kind], text: otherText(p) })),
  };
}
