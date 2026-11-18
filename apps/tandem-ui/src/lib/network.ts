// La pantalla Red (`V3-Network`) sin DOM: del estado de la red del agente a lo que se dibuja (la
// gráfica de 60 s, los indicadores, la nota del último cambio). Puro, para probarlo en vitest.

import { LINK_NAME } from "./machines";
import { EMPTY_NETWORK, INPUT_TARGET_MS, type LatencyPoint, type LinkKind, type NetworkView, type PathState, type PathSwitchView, type PathView } from "./store/types";

export const HISTORY_SECS = 60;

/** El orden fijo de los caminos (el de prioridad, DESIGN §6). */
export const LINKS: LinkKind[] = ["usb4", "ethernet", "wifi"];

// ---------------------------------------------------------------------------------------
// Lo que llega del núcleo de la app (`tandem_ui::session::NetworkView`, evento `agent-network`).

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

/** Bytes por segundo → Gbps (con dos decimales). */
export function gbpsOf(bytesPerSec: number | null | undefined): number | null {
  return typeof bytesPerSec === "number" ? Math.round(((bytesPerSec * 8) / 1e9) * 100) / 100 : null;
}

/** El estado del agente como lo dibuja la UI. `now`: cuándo llegó (milisegundos Unix). */
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
// Textos.

export const STATE_LABEL: Record<PathState, string> = { primary: "Main", backup: "Backup", unplugged: "No cable", down: "Down" };

/** Un camino sin respuesta: el Wi-Fi no tiene cable, así que no puede estar «no cable». */
export const unpluggedText = (kind: LinkKind): string => (kind === "wifi" ? "no signal" : "no cable");

/** El rótulo del estado de un camino («Principal», «No cable», «Sin señal»…). */
export function stateLabel(p: Pick<PathView, "kind" | "state">): string {
  if (p.state === "unplugged") return capitalize(unpluggedText(p.kind));
  return STATE_LABEL[p.state];
}
const capitalize = (t: string) => t.charAt(0).toUpperCase() + t.slice(1);

/** «0.3 ms», «14 ms»; «—» sin dato. */
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

/** Lo que se avisa al terminar la prueba: «USB-C: 9.4 Gbps». */
export function measuredText(kind: LinkKind, gbps: number): string {
  return `${LINK_NAME[kind]}: ${formatRate(gbps)}`;
}

/** «0.6 s». */
export function formatSecs(millis: number): string {
  return `${(millis / 1000).toFixed(1)} s`;
}

/** Un camino responde: lleva el tráfico o espera de respaldo. */
export function isUp(p: PathView): boolean {
  return p.state === "primary" || p.state === "backup";
}

export function primaryOf(n: NetworkView): PathView | null {
  return n.paths.find((p) => p.state === "primary") ?? null;
}

/** La descripción de un camino para lectores de pantalla («USB-C activo a 0.3 ms»). */
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
// Barras de las tarjetas por camino.

/** La latencia de las barras se mide contra 10 ms. */
export const LATENCY_SCALE_MS = 10;
/** Lo que rinde cada camino a plena marcha, en Gbps: la barra de ancho de banda va contra eso. */
export const CAPACITY_GBPS: Record<LinkKind, number> = { usb4: 10, ethernet: 2.5, wifi: 3.4 };
/** El anillo de ancho de banda se llena con 10 Gbps. */
export const RING_GBPS = 10;

const pct = (v: number) => Math.max(0, Math.min(100, Math.round(v * 100)));

export function latencyBar(rttMs: number | null): number {
  return rttMs === null ? 0 : pct(rttMs / LATENCY_SCALE_MS);
}

export function bandwidthBar(kind: LinkKind, gbps: number | null): number {
  return gbps === null ? 0 : pct(gbps / CAPACITY_GBPS[kind]);
}

/** Colores de las barras de un camino: el principal verde y azul, el de respaldo ámbar. */
export function barColors(state: PathState): { latency: string; bandwidth: string } {
  return state === "primary" ? { latency: "#6cc59a", bandwidth: "#7aa7e6" } : { latency: "#d9a75a", bandwidth: "#d9a75a" };
}

// ---------------------------------------------------------------------------------------
// Indicadores.

export type Tone = "ok" | "warn" | "bad";

export interface LatencyKpi {
  p50: number | null;
  p99: number | null;
  /** 0–1: qué tanto del objetivo usa el p99. */
  load: number;
  tone: Tone;
}

/** El anillo de latencia: p99 de la entrada contra el objetivo del camino principal. */
export function latencyKpi(n: NetworkView): LatencyKpi {
  const p = primaryOf(n);
  const p99 = p?.inputP99Ms ?? null;
  const load = p && p99 !== null ? Math.max(0, Math.min(1, p99 / p.targetMs)) : 0;
  return { p50: p?.inputP50Ms ?? null, p99, load, tone: load >= 1 ? "bad" : load >= 0.8 ? "warn" : "ok" };
}

export interface BandwidthKpi {
  gbps: number | null;
  /** El camino medido (el principal). */
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
// La gráfica de 60 s.

export const CHART = { width: 400, height: 74 } as const;

/**
 * La latencia de la entrada de cada uno de los últimos 60 s, del más viejo al más nuevo (los
 * que faltan al arrancar quedan `null` to the left of). En cada segundo vale la del camino que
 * la llevaba; si en todo el historial no hubo entrada, el ida y vuelta de los pings.
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
  /** La línea, en tramos (un hueco sin muestras corta la línea). */
  line: string;
  /** El área bajo la línea. */
  area: string;
  targetMs: number | null;
  /** La raya del objetivo. */
  targetY: number | null;
  /** El pico, si pasó el objetivo. */
  peak: { x: number; y: number; ms: number } | null;
  /** Hay al menos una muestra. */
  hasData: boolean;
}

const r2 = (v: number) => Math.round(v * 100) / 100;

/**
 * Los puntos de la gráfica en el lienzo de `V3-Network` (400×74): la escala deja el objetivo a
 * 46 % de la altura (4 ms en y = 34) y crece solo si el pico lo pide.
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

/** El objetivo de la gráfica: el del camino que lleva la entrada (o, sin él, el de respaldo). */
export function chartTarget(n: NetworkView): number | null {
  return (primaryOf(n) ?? n.paths.find((p) => p.state === "backup"))?.targetMs ?? null;
}

export interface PathNote {
  /** `warn` si hubo un corte; `info` si el camino cambió sin corte. */
  tone: "warn" | "info";
  text: string;
}

/**
 * La nota bajo la gráfica: el último corte de los 60 s de la gráfica («Pico al desconectar el
 * cable · pasó a Wi-Fi en 0.6 s») o, sin cortes, el último cambio. Sin cambios, nada.
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
