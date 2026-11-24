// The Network screen (`V3-Network`) without a DOM: from the agent's network state to what is drawn (the
// 60 s chart, the indicators, the note of the last change). Pure, to test in vitest.

import { LINK_NAME } from "./machines";
import { EMPTY_NETWORK, INPUT_TARGET_MS, type LatencyPoint, type LinkKind, type LinkView, type NetworkView, type PathState, type PathSwitchView, type PathView } from "./store/types";

export const HISTORY_SECS = 60;

/** The fixed order of the paths (the priority one, DESIGN §6). */
export const LINKS: LinkKind[] = ["usb4", "ethernet", "wifi"];

// ---------------------------------------------------------------------------------------
// What arrives from the app core (`tandem_ui::session::NetworkView`, `agent-network` event).

export interface NetworkWire {
  paths: {
    link: LinkKind;
    state: PathState;
    rtt_us: number | null;
    input_p50_us: number | null;
    input_p99_us: number | null;
    input_target_us: number;
    bytes_per_sec: number | null;
    history: { rtt_us: number | null; input_p99_us: number | null }[];
  }[];
  switches: { at_ms: number; from: LinkKind | null; to: LinkKind | null; took_ms: number | null }[];
  measuring: LinkKind | null;
  notices?: { interface: string; message: string; command: string }[];
}

const ms = (us: number | null | undefined): number | null => (typeof us === "number" ? Math.round(us / 10) / 100 : null);

/** Bytes per second → Gbps (with two decimals). */
export function gbpsOf(bytesPerSec: number | null | undefined): number | null {
  return typeof bytesPerSec === "number" ? Math.round(((bytesPerSec * 8) / 1e9) * 100) / 100 : null;
}

/** The agent's state as the UI draws it. `now`: when it arrived (Unix milliseconds). */
export function networkFromWire(w: NetworkWire, now: number): NetworkView {
  const paths = LINKS.map((kind): PathView => {
    const p = w.paths.find((x) => x.link === kind);
    if (!p) return { ...EMPTY_NETWORK.paths.find((x) => x.kind === kind)! };
    return {
      kind,
      state: p.state,
      rttMs: ms(p.rtt_us),
      inputP50Ms: ms(p.input_p50_us),
      inputP99Ms: ms(p.input_p99_us),
      targetMs: ms(p.input_target_us) ?? INPUT_TARGET_MS[kind],
      gbps: gbpsOf(p.bytes_per_sec),
      history: p.history.map((h): LatencyPoint => ({ rttMs: ms(h.rtt_us), inputP99Ms: ms(h.input_p99_us) })),
      detail: null,
    };
  });
  return {
    paths,
    switches: w.switches.map((s): PathSwitchView => ({ at: s.at_ms, from: s.from, to: s.to, tookMs: s.took_ms })),
    measuring: w.measuring,
    notices: w.notices ?? [],
    now,
  };
}

// ---------------------------------------------------------------------------------------
// Texts.

export const STATE_LABEL: Record<PathState, string> = { primary: "Primary", backup: "Backup", unplugged: "No cable", down: "Down" };

/** A path with no reply: Wi-Fi has no cable, so it cannot be "No cable". */
export const unpluggedText = (kind: LinkKind): string => (kind === "wifi" ? "no signal" : "no cable");

/** The label of a path's state ("Primary", "No cable", "No signal"…). */
export function stateLabel(p: Pick<PathView, "kind" | "state">): string {
  if (p.state === "unplugged") return capitalize(unpluggedText(p.kind));
  return STATE_LABEL[p.state];
}
const capitalize = (t: string) => t.charAt(0).toUpperCase() + t.slice(1);

/** "0.3 ms", "14 ms"; "—" without data. */
export function formatMs(v: number | null): string {
  if (v === null) return "—";
  if (v > 0 && v < 0.1) return "<0.1 ms";
  return `${v < 10 ? Math.round(v * 10) / 10 : Math.round(v)} ms`;
}

/** «9.4 Gbps», «410 Mbps»; «—» not measured. */
export function formatRate(gbps: number | null): string {
  if (gbps === null) return "—";
  if (gbps >= 1) return `${Math.round(gbps * 10) / 10} Gbps`;
  return `${Math.round(gbps * 1000)} Mbps`;
}

/** What is announced when the test ends: "USB-C: 9.4 Gbps". */
export function measuredText(kind: LinkKind, gbps: number): string {
  return `${LINK_NAME[kind]}: ${formatRate(gbps)}`;
}

/** «0.6 s». */
export function formatSecs(millis: number): string {
  return `${(millis / 1000).toFixed(1)} s`;
}

/** A path responds: it carries the traffic or waits as a backup. */
export function isUp(p: PathView): boolean {
  return p.state === "primary" || p.state === "backup";
}

export function primaryOf(n: NetworkView): PathView | null {
  return n.paths.find((p) => p.state === "primary") ?? null;
}

/** The newest value of a path's history that is not a gap. */
function newest(p: PathView, of: (h: LatencyPoint) => number | null): number | null {
  for (let i = p.history.length - 1; i >= 0; i--) {
    const h = p.history[i];
    const v = h ? of(h) : null;
    if (v !== null) return v;
  }
  return null;
}

/**
 * The Desk's link summary (T-356) from the agent's real measurements of the path carrying the
 * input: the round trip of the link pings, the input latency the session measures (capture →
 * injection, p99) and the bandwidth of the last test. A second without input leaves no p99, so
 * the newest one of the last 60 s is kept; `null` when nothing was measured at all.
 */
export function linkSummary(kind: LinkKind | null, network: NetworkView): LinkView {
  const p = kind ? (network.paths.find((x) => x.kind === kind && x.state === "primary") ?? null) : null;
  if (!p) return { kind, gbps: null, ms: null, inputP99: null };
  return { kind, gbps: p.gbps, ms: p.rttMs ?? newest(p, (h) => h.rttMs), inputP99: p.inputP99Ms ?? newest(p, (h) => h.inputP99Ms) };
}

/** The Desk's link figures: never a dash; what was not measured yet reads 0 (and says so in `title`). */
export function linkFigures(l: LinkView): { rate: string; input: string; title: string } {
  const gbps = l.gbps === null ? "0 Gbps" : formatRate(l.gbps);
  const ms = l.ms === null ? "0 ms" : formatMs(l.ms);
  const missing = [l.gbps === null && "bandwidth (measure it on Network)", l.ms === null && "latency", l.inputP99 === null && "input latency"].filter(Boolean);
  return {
    rate: `${gbps} · ${ms}`,
    input: `p99 ${l.inputP99 === null ? "0 ms" : formatMs(l.inputP99)}`,
    title: missing.length > 0 ? `Not measured yet: ${missing.join(", ")}` : "",
  };
}

/** The description of a path for screen readers ("USB-C active at 0.3 ms"). */
export function describePath(p: PathView): string {
  const name = LINK_NAME[p.kind];
  switch (p.state) {
    case "primary":
      return `${name} active${p.rttMs !== null ? ` at ${formatMs(p.rttMs)}` : ""}`;
    case "backup":
      return `${name} as backup${p.rttMs !== null ? ` at ${formatMs(p.rttMs)}` : ""}`;
    case "unplugged":
      return `${name} ${unpluggedText(p.kind)}`;
    case "down":
      return `${name} down`;
  }
}

// ---------------------------------------------------------------------------------------
// Bars of the per-path cards.

/** The latency of the bars is measured against 10 ms. */
export const LATENCY_SCALE_MS = 10;
/** What each path yields at full speed, in Gbps: the bandwidth bar goes against that. */
export const CAPACITY_GBPS: Record<LinkKind, number> = { usb4: 10, ethernet: 2.5, wifi: 3.4 };
/** The bandwidth ring fills at 10 Gbps. */
export const RING_GBPS = 10;

const pct = (v: number) => Math.max(0, Math.min(100, Math.round(v * 100)));

export function latencyBar(rttMs: number | null): number {
  return rttMs === null ? 0 : pct(rttMs / LATENCY_SCALE_MS);
}

export function bandwidthBar(kind: LinkKind, gbps: number | null): number {
  return gbps === null ? 0 : pct(gbps / CAPACITY_GBPS[kind]);
}

/** Colors of a path's bars: the primary one green and blue, the backup one amber. */
export function barColors(state: PathState): { latency: string; bandwidth: string } {
  return state === "primary" ? { latency: "#6cc59a", bandwidth: "#7aa7e6" } : { latency: "#d9a75a", bandwidth: "#d9a75a" };
}

// ---------------------------------------------------------------------------------------
// Indicators.

export type Tone = "ok" | "warn" | "bad";

export interface LatencyKpi {
  p50: number | null;
  p99: number | null;
  /** 0–1: how much of the target the p99 uses. */
  load: number;
  tone: Tone;
}

/** The latency ring: p99 of the input against the target of the primary path. */
export function latencyKpi(n: NetworkView): LatencyKpi {
  const p = primaryOf(n);
  const p99 = p?.inputP99Ms ?? null;
  const load = p && p99 !== null ? Math.max(0, Math.min(1, p99 / p.targetMs)) : 0;
  return { p50: p?.inputP50Ms ?? null, p99, load, tone: load >= 1 ? "bad" : load >= 0.8 ? "warn" : "ok" };
}

export interface BandwidthKpi {
  gbps: number | null;
  /** The measured path (the primary one). */
  via: LinkKind | null;
  /** 0–1. */
  fill: number;
}

export function bandwidthKpi(n: NetworkView): BandwidthKpi {
  const p = primaryOf(n);
  const gbps = p?.gbps ?? null;
  return { gbps, via: p?.kind ?? null, fill: gbps === null ? 0 : Math.round(Math.max(0, Math.min(1, gbps / RING_GBPS)) * 1000) / 1000 };
}

// ---------------------------------------------------------------------------------------
// The 60 s chart.

export const CHART = { width: 400, height: 74 } as const;

/**
 * The input latency of each of the last 60 s, from oldest to newest (those
 * missing at startup are `null` on the left). In each second it is that of the path that
 * carried it; if the whole history had no input, the ping round trip.
 */
export function latencySeries(n: NetworkView): (number | null)[] {
  const order = [...n.paths].sort((a, b) => Number(b.state === "primary") - Number(a.state === "primary"));
  const pick = (field: "inputP99Ms" | "rttMs"): (number | null)[] =>
    Array.from({ length: HISTORY_SECS }, (_, slot) => {
      for (const p of order) {
        const v = p.history[slot - (HISTORY_SECS - p.history.length)]?.[field] ?? null;
        if (v !== null) return v;
      }
      return null;
    });
  const input = pick("inputP99Ms");
  return input.some((v) => v !== null) ? input : pick("rttMs");
}

export interface ChartGeometry {
  /** The line, in stretches (a gap without samples cuts the line). */
  line: string;
  /** The area under the line. */
  area: string;
  targetMs: number | null;
  /** The target line. */
  targetY: number | null;
  /** The peak, if it went past the target. */
  peak: { x: number; y: number; ms: number } | null;
  /** There is at least one sample. */
  hasData: boolean;
}

const r2 = (v: number) => Math.round(v * 100) / 100;

/**
 * The chart points on the `V3-Network` canvas (400×74): the scale puts the target at
 * 46 % of the height (4 ms at y = 34) and grows only if the peak asks for it.
 */
export function chartGeometry(series: (number | null)[], targetMs: number | null): ChartGeometry {
  const { width, height } = CHART;
  const values = series.filter((v): v is number => v !== null);
  const top = Math.max(values.length ? Math.max(...values) * 1.05 : 0, (targetMs ?? 0) * 1.85, 1);
  const y = (v: number) => r2(height - Math.max(0, Math.min(1, v / top)) * height);
  const step = width / (HISTORY_SECS - 1);
  const x = (i: number) => r2(i * step);

  let line = "";
  let area = "";
  let run: [number, number][] = [];
  const flush = () => {
    if (run.length > 0) {
      const d = run.map(([px, py], k) => `${k === 0 ? "M" : "L"}${px} ${py}`).join(" ");
      line += `${line ? " " : ""}${d}`;
      area += `${area ? " " : ""}${d} L${run[run.length - 1]![0]} ${height} L${run[0]![0]} ${height}Z`;
    }
    run = [];
  };
  series.forEach((v, i) => {
    if (v === null) flush();
    else run.push([x(i), y(v)]);
  });
  flush();

  let peak: ChartGeometry["peak"] = null;
  if (values.length > 0) {
    const max = Math.max(...values);
    if (targetMs !== null && max > targetMs) {
      const i = series.indexOf(max);
      peak = { x: x(i), y: y(max), ms: max };
    }
  }
  return { line, area, targetMs, targetY: targetMs === null ? null : y(targetMs), peak, hasData: values.length > 0 };
}

/** The chart target: that of the path carrying the input (or, without it, the backup one). */
export function chartTarget(n: NetworkView): number | null {
  return (primaryOf(n) ?? n.paths.find((p) => p.state === "backup"))?.targetMs ?? null;
}

export interface PathNote {
  /** `warn` if there was a cut; `info` if the path changed without a cut. */
  tone: "warn" | "info";
  text: string;
}

/**
 * The note under the chart: the last cut of the chart's 60 s ("Peak when the
 * cable was unplugged · switched to Wi-Fi in 0.6 s") or, without cuts, the last change. Without changes, nothing.
 */
export function pathNote(n: NetworkView, hasPeak: boolean): PathNote | null {
  const recent = n.switches.filter((s) => s.at >= n.now - HISTORY_SECS * 1000);
  const cut = recent.find((s) => s.tookMs !== null);
  if (cut) {
    const cause = cut.from === "wifi" ? (hasPeak ? "Spike when Wi-Fi dropped" : "Wi-Fi dropped") : hasPeak ? "Spike when the cable was unplugged" : "The cable was unplugged";
    const after = cut.to ? `switched to ${LINK_NAME[cut.to]} in ${formatSecs(cut.tookMs ?? 0)}` : `no path for ${formatSecs(cut.tookMs ?? 0)}`;
    return { tone: "warn", text: `${cause} · ${after}` };
  }
  const last = recent[0];
  if (!last?.to) return null;
  return { tone: "info", text: last.from ? `Switched from ${LINK_NAME[last.from]} to ${LINK_NAME[last.to]}` : `Connected over ${LINK_NAME[last.to]}` };
}
