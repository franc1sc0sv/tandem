// «Changes in progress» (`F-Transactions`): una transacción con sus pasos, el tiempo de cada
// uno y la vuelta atrás visible. Puro: la pantalla solo dibuja `viewOf`.

import { advance, type Progress } from "./machines";
import { INPUT_LABEL, shortMonitor } from "./monitors";
import type { Machine, Step, TxnView } from "./model";
import type { MonitorUi } from "./store/types";

export interface Run {
  /** Título fijo si lo puso quien pidió el cambio («Applying the scene “Game”»); si no, sale de los pasos. */
  title: string | null;
  /** Lo que se dice al terminar bien («Scene “Game” active.»). */
  doneText: string | null;
  progress: Progress;
  startedAt: number;
  endedAt: number | null;
  /** Hora en que empezó y terminó cada paso (milisegundos). */
  stepStart: (number | undefined)[];
  stepEnd: (number | undefined)[];
}

export interface Origin {
  title: string;
  doneText: string;
}

/** Un `started` abre la corrida; los demás avisos de la misma transacción la avanzan. */
export function feedRun(run: Run | null, e: TxnView, at: number, origin: Origin | null = null): Run | null {
  if (e.type === "started") {
    const progress = advance(null, e);
    if (!progress) return run;
    return {
      title: origin?.title ?? null,
      doneText: origin?.doneText ?? null,
      progress,
      startedAt: at,
      endedAt: null,
      stepStart: e.steps.map(() => undefined),
      stepEnd: e.steps.map(() => undefined),
    };
  }
  if (!run || run.progress.txn !== e.txn) return run;
  const progress = advance(run.progress, e) ?? run.progress;
  const stepStart = [...run.stepStart];
  const stepEnd = [...run.stepEnd];
  if (e.type === "step_started") stepStart[e.index] = at;
  if (e.type === "step_done" || e.type === "step_failed" || e.type === "step_undone") stepEnd[e.index] = at;
  const over = e.type === "committed" || e.type === "failed";
  return { ...run, progress, stepStart, stepEnd, endedAt: over ? at : run.endedAt };
}

export const running = (r: Run | null): boolean => r !== null && r.endedAt === null;

export type RowIcon = "done" | "run" | "bad" | "todo" | "back";

export interface Row {
  key: string;
  icon: RowIcon;
  text: string;
  detail: string;
  /** `bad`: el paso que falló. */
  tone: "" | "bad";
}

export interface Message {
  text: string;
  tone: "mut" | "ok" | "warn";
}

export interface RunView {
  title: string;
  /** `1.2 s`. */
  elapsed: string;
  rows: Row[];
  message: Message;
  outcome: "running" | "ok" | "failed";
}

export interface Names {
  machine: (m: Machine) => string;
  monitors: MonitorUi[];
}

const seconds = (ms: number) => `${(Math.max(0, ms) / 1000).toFixed(1)} s`;
const other = (m: Machine): Machine => (m === "a" ? "b" : "a");

/** El monitor de un paso, con el nombre corto («34"»). */
function monitorName(id: string, names: Names): string {
  const m = names.monitors.find((x) => x.id === id);
  return m ? shortMonitor(m.name) : id;
}

/** `MonitorInput` en la forma larga que usa el agente en sus errores. */
const LONG: Record<string, string> = { DisplayPort: "DisplayPort", HDMI: "HDMI" };

/** El título de lo que se está haciendo, a partir de sus pasos. */
export function titleOf(steps: Step[], names: Names): string {
  const first = steps[0];
  if (!first) return "Change in progress";
  if (steps.some((s) => s.step === "show_machine" || s.step === "arrange" || s.step === "geometry")) return "Applying a scene";
  if (first.step === "power_on") return `Powering on ${names.machine(first.machine)}`;
  if (first.step === "power_off") return `Powering off ${names.machine(first.machine)}`;
  if (first.step === "switch_monitor") return `Changing the input of ${monitorName(first.monitor, names)}`;
  return "Change in progress";
}

function label(s: Step, steps: Step[], names: Names): string {
  const m = names.machine;
  switch (s.step) {
    case "power_on":
      return `Power on ${m(s.machine)}`;
    case "power_off":
      return `Power off ${m(s.machine)}`;
    case "switch_monitor":
      return `${monitorName(s.monitor, names)} → ${INPUT_LABEL[s.input]}`;
    case "show_machine": {
      const port = names.monitors.find((x) => x.id === s.monitor)?.ports[s.machine];
      return `${monitorName(s.monitor, names)} → ${m(s.machine)}${port ? ` (${INPUT_LABEL[port]})` : ""}`;
    }
    case "arrange":
      return "Arrange the desktops";
    case "geometry": {
      const show = steps.find((x): x is Extract<Step, { step: "show_machine" }> => x.step === "show_machine");
      return show ? `Mouse: take the ${monitorName(show.monitor, names)} off ${m(other(show.machine))}` : "Keyboard and mouse: new geometry";
    }
  }
}

const isMonitorStep = (s: Step) => s.step === "show_machine" || s.step === "switch_monitor";

/** El error de una entrada que no dio imagen (lo que dice el agente) en una frase corta. */
function noImage(error: string): string | null {
  const m = /(?:no mostró imagen en|showed no picture (?:on|over|in)) (\S+?)(?:;|$)/.exec(error);
  return m ? (LONG[m[1]] ?? m[1]) : null;
}

const sentence = (s: string) => (s ? s[0].toUpperCase() + s.slice(1) : s);

export function viewOf(run: Run, names: Names, now: number): RunView {
  const { progress: p } = run;
  const outcome = p.outcome.kind === "committed" ? "ok" : p.outcome.kind === "failed" ? "failed" : "running";
  const failedAt = p.status.findIndex((s) => s === "failed");
  const rows: Row[] = [{ key: "save", icon: "done", text: "Save how everything is", detail: "done", tone: "" }];
  p.steps.forEach((s, i) => {
    // Lo que no llegó a empezar antes de un fallo no se muestra: lo reemplaza la vuelta atrás.
    if (failedAt >= 0 && i > failedAt) return;
    const st = p.status[i];
    const took = run.stepStart[i] !== undefined && run.stepEnd[i] !== undefined ? seconds((run.stepEnd[i] ?? 0) - (run.stepStart[i] ?? 0)) : "";
    const text = label(s, p.steps, names);
    const key = `s${i}`;
    if (st === "done") rows.push({ key, icon: "done", text, detail: isMonitorStep(s) ? (took ? `confirmed · ${took}` : "confirmed") : "done", tone: "" });
    else if (st === "undone") rows.push({ key, icon: "done", text, detail: "undone", tone: "" });
    else if (st === "run") rows.push({ key, icon: "run", text, detail: isMonitorStep(s) ? "waiting for the monitor" : "", tone: "" });
    else if (st === "failed") {
      const verdict = isMonitorStep(s) && noImage(p.error ?? "") !== null;
      rows.push({ key, icon: "bad", text, detail: verdict ? "the monitor said: change failed" : (p.error ?? "failed"), tone: "bad" });
    } else rows.push({ key, icon: "todo", text, detail: "", tone: "" });
  });
  if (failedAt >= 0) {
    const bad = p.steps[failedAt];
    const was = bad && isMonitorStep(bad) ? names.monitors.find((x) => x.id === (bad as { monitor: string }).monitor) : null;
    const back = was?.shows ? `${shortMonitor(was.name)} → ${names.machine(was.shows)}` : "";
    rows.push({ key: "back", icon: outcome === "running" ? "back" : "done", text: "Rolling back to how it was", detail: back, tone: "" });
  }

  let message: Message;
  if (outcome === "ok") message = { text: run.doneText ?? "Done.", tone: "ok" };
  else if (failedAt >= 0) {
    const sig = noImage(p.error ?? "");
    const why = sig ? `There was no ${sig} signal` : sentence((p.error ?? "").replace(/\.$/, ""));
    message = { text: `${why ? `${why}. ` : ""}Everything goes back to how it was; nothing was left half done.`, tone: "warn" };
  } else message = { text: "The monitor confirms each change; nothing is guessed.", tone: "mut" };

  return { title: run.title ?? titleOf(p.steps, names), elapsed: seconds((run.endedAt ?? now) - run.startedAt), rows, message, outcome };
}
