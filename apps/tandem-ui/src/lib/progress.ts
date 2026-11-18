// Las otras dos tarjetas de «Cambios en progreso» (`F-Transacciones`): el encendido de una
// máquina y la medición de un enlace. Puro: la pantalla solo dibuja lo que sale de aquí.

import { LINK_NAME } from "./machines";
import type { Machine } from "./model";
import { formatMs, formatRate, RING_GBPS } from "./network";
import type { LinkKind, NetworkView, PathView } from "./store/types";
import type { Run } from "./txn";

/** Lo que se espera a que el agente de la máquina encendida conteste (DESIGN §3.6). */
export const BOOT_WAIT_MS = 120_000;
/** Cada cuánto se reenvía el paquete Wake-on-LAN. */
export const WOL_EVERY_S = 5;

export type StepIcon = "done" | "run" | "bad" | "todo";

export interface EnergyRow {
  key: string;
  icon: StepIcon;
  text: string;
  detail: string;
  /** El detalle va en monoespaciada (el reloj). */
  mono: boolean;
  /** Fila apagada (lo que aún no empezó). */
  dim: boolean;
}

export interface EnergyView {
  title: string;
  rows: EnergyRow[];
  /** Mientras espera se puede dejar de esperar. */
  waiting: boolean;
}

/** `0:18`. */
export function clockText(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** La máquina que se está encendiendo en esta corrida, si lo es. */
export function powerTarget(run: Run | null): Machine | null {
  const s = run?.progress.steps[0];
  return s?.step === "power_on" ? s.machine : null;
}

/** La tarjeta de energía de una corrida `power_on`; `null` si la corrida es de otra cosa. */
export function energyView(run: Run | null, name: (m: Machine) => string, from: string, now: number): EnergyView | null {
  const target = powerTarget(run);
  if (!run || !target) return null;
  const st = run.progress.status[0];
  const waited = clockText((run.endedAt ?? now) - run.startedAt);
  const rows: EnergyRow[] = [
    { key: "wol", icon: "done", text: `Wake-on-LAN enviado desde ${from}`, detail: st === "done" ? "listo" : `cada ${WOL_EVERY_S} s`, mono: false, dim: false },
    {
      key: "wait",
      icon: st === "done" ? "done" : st === "failed" ? "bad" : "run",
      text: "Esperando a que su agente conteste",
      detail: st === "done" ? waited : st === "failed" ? (run.progress.error ?? "no contestó") : `${waited} / ${clockText(BOOT_WAIT_MS)}`,
      mono: st !== "failed",
      dim: false,
    },
    { key: "join", icon: st === "done" ? "done" : "todo", text: "Volver a incluirla en el puesto", detail: "", mono: false, dim: st !== "done" },
  ];
  return { title: `Encendiendo ${name(target)}`, rows, waiting: st === "run" || st === "todo" };
}

export interface MeterRow {
  name: string;
  text: string;
}

export interface MeasureView {
  kind: LinkKind;
  title: string;
  /** El número grande: «9.4» y su unidad. */
  value: string;
  unit: string;
  latency: string;
  /** 0–100: lo medido contra la escala de la barra. */
  percent: number;
  /** `9.4 de 10 Gbps`, para lectores de pantalla. */
  aria: string;
  caption: string;
  others: MeterRow[];
}

const CAPTION: Record<LinkKind, string> = {
  usb4: "USB4 20 Gbps de enlace",
  ethernet: "Ethernet 2.5 Gbps de enlace",
  wifi: "Wi-Fi: enlace negociado",
};

const otherText = (p: PathView): string => {
  if (p.state === "unplugged") return "sin cable";
  if (p.state === "down") return "caído";
  return [p.gbps === null ? null : formatRate(p.gbps), p.rttMs === null ? null : formatMs(p.rttMs)].filter(Boolean).join(" · ") || "—";
};

/** El camino que se mide ahora, o el último medido; `null` si no hay medición que mostrar. */
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
    title: `Midiendo el enlace ${LINK_NAME[kind]}`,
    value,
    unit,
    latency: path.rttMs === null ? "—" : formatMs(path.rttMs),
    percent,
    aria: `${g === null ? "sin medir" : `${Math.round(g * 10) / 10}`} de ${RING_GBPS} Gbps`,
    caption: CAPTION[kind],
    others: n.paths.filter((p) => p.kind !== kind).map((p) => ({ name: LINK_NAME[p.kind], text: otherText(p) })),
  };
}
