// «Agregar monitor» (T-080): qué dice cada monitor detectado y cómo va su calibración, a
// partir de lo que manda el agente. Puro, para probarlo sin DOM.

import type { CalibrationOutcome, CalibrationView, Desk, DisplayProfile, DisplayView, Input } from "./model";
import { PORT_NAME, hex } from "./probe";

export const INPUTS: Input[] = ["hdmi", "displayport"];
export const SHORT: Record<Input, string> = { hdmi: "HDMI", displayport: "DP" };

/** Cómo va una entrada: esperando, cambio a mano, probando códigos, encontrada o sin código. */
export type LaneState = "waiting" | "switch" | "testing" | "found" | "failed";

export interface Lane {
  input: Input;
  state: LaneState;
  /** El código encontrado, o el que se está probando. */
  code: number | null;
  attempt: number;
  of: number;
}

export interface Calibration {
  display: number;
  name: string;
  monitor: string | null;
  /** La entrada de partida: la calibración siempre vuelve a ella. */
  start: Input;
  lanes: Record<Input, Lane>;
  /** Lo que está haciendo ahora, para el título. */
  now: "reading" | "switch" | "testing" | "returning" | "verifying";
  /** La entrada del cambio a mano o de las pruebas en curso. */
  input: Input | null;
  outcome: CalibrationOutcome | null;
}

function lane(input: Input): Lane {
  return { input, state: "waiting", code: null, attempt: 0, of: 0 };
}

/** El avance siguiente. Un aviso suelto sin `started` antes no arma nada. */
export function advance(run: Calibration | null, ev: CalibrationView): Calibration | null {
  if (ev.type === "started") {
    return {
      display: ev.display,
      name: ev.name,
      monitor: ev.monitor,
      start: ev.start,
      lanes: { hdmi: lane("hdmi"), displayport: lane("displayport") },
      now: "reading",
      input: null,
      outcome: null,
    };
  }
  if (!run || run.outcome) return run;
  const lanes = { hdmi: { ...run.lanes.hdmi }, displayport: { ...run.lanes.displayport } };
  const next: Calibration = { ...run, lanes };
  if (ev.type === "finished") {
    next.outcome = ev.outcome;
    // Sin código queda la entrada que se estaba probando; las demás, como estaban.
    for (const l of Object.values(lanes)) {
      if (l.state === "found") continue;
      l.state = ev.outcome.kind === "failed" && l.state === "testing" ? "failed" : "waiting";
    }
    if (ev.outcome.kind === "saved") {
      lanes.hdmi = { ...lanes.hdmi, state: "found", code: ev.outcome.hdmi };
      lanes.displayport = { ...lanes.displayport, state: "found", code: ev.outcome.display_port };
    }
    return next;
  }
  const step = ev.step;
  switch (step.kind) {
    case "reading":
      next.now = "reading";
      break;
    case "user_switch":
      next.now = "switch";
      next.input = step.input;
      if (lanes[step.input].state !== "found") lanes[step.input].state = "switch";
      break;
    case "testing":
      next.now = "testing";
      next.input = step.input;
      // La entrada a la que se pasó a mano ya se vio: queda esperando sus pruebas.
      for (const l of Object.values(lanes)) if (l.state === "switch") l.state = "waiting";
      lanes[step.input].state = "testing";
      break;
    case "trying":
      next.now = "testing";
      next.input = step.input;
      lanes[step.input] = { ...lanes[step.input], state: "testing", code: step.code, attempt: step.attempt, of: step.of };
      break;
    case "seen":
      break;
    case "returned":
      next.now = "returning";
      break;
    case "found":
      lanes[step.input] = { ...lanes[step.input], state: "found", code: step.code };
      break;
    case "verifying":
      next.now = "verifying";
      next.input = null;
      break;
    case "verified":
      break;
  }
  return next;
}

/** El renglón de una entrada. */
export function laneText(l: Lane): string {
  switch (l.state) {
    case "waiting":
      return "en espera";
    case "switch":
      return "cámbialo desde el menú del monitor";
    case "testing":
      return l.code === null ? "probando…" : `probando ${hex(l.code)} · ${l.attempt} de ${l.of}`;
    case "found":
      return l.code === null ? "✓" : `✓ ${hex(l.code)}`;
    case "failed":
      return "ningún código funcionó";
  }
}

/** El título de la calibración en curso. */
export function headline(run: Calibration, label: string): string {
  if (run.outcome) {
    switch (run.outcome.kind) {
      case "saved":
        return result(label, true, true);
      case "failed":
        return `No se pudo calibrar el ${label}`;
      case "cancelled":
        return "Calibración cancelada";
    }
  }
  switch (run.now) {
    case "reading":
      return `Leyendo el ${label}…`;
    case "switch":
      return run.input === run.start
        ? `Regresa el ${label} a ${PORT_NAME[run.start]} desde su menú`
        : `Pasa el ${label} a ${PORT_NAME[run.input ?? "hdmi"]} desde su menú`;
    case "testing":
      return `Probando códigos de ${PORT_NAME[run.input ?? "hdmi"]}…`;
    case "returning":
      return `Volviendo a ${PORT_NAME[run.start]}…`;
    case "verifying":
      return "Verificando: ida y vuelta con los dos códigos…";
  }
}

/** Qué hacer ahora, debajo del título. */
export function hint(run: Calibration): string {
  if (run.outcome) {
    switch (run.outcome.kind) {
      case "saved":
        return `${run.outcome.attempts} códigos probados. El próximo cambio de modo ya los usa.`;
      case "failed":
        return `${capitalize(run.outcome.reason)}. El monitor volvió a ${PORT_NAME[run.start]}.`;
      case "cancelled":
        return `El monitor volvió a ${PORT_NAME[run.start]}. No se guardó nada.`;
    }
  }
  if (run.now === "switch") return "Cuando veas la otra máquina en ese monitor, haz clic en su aviso o di aquí qué ves.";
  return `Después de cada código, di qué ves en el monitor: la Mac, la PC o negro (o haz clic en «¿Ves esta pantalla?»). Siempre vuelve a ${PORT_NAME[run.start]}.`;
}

/** «34": HDMI ✓ DP ✓». */
export function result(label: string, hdmi: boolean, dp: boolean): string {
  return `${label}: HDMI ${hdmi ? "✓" : "✗"} DP ${dp ? "✓" : "✗"}`;
}

/** Cómo se llama un monitor en la ventana: su etiqueta de la configuración o su modelo. */
export function labelOf(desk: Desk | null, monitor: string | null, name: string): string {
  const m = monitor && desk?.monitors.find((x) => x.id === monitor);
  return m ? m.label : name;
}

export interface Badge {
  text: string;
  kind: "ok" | "known" | "none";
  codes: string | null;
}

export function badge(p: DisplayProfile): Badge {
  const codes = (hdmi: number, dp: number) => `HDMI ${hex(hdmi)} · DP ${hex(dp)}`;
  switch (p.kind) {
    case "configured":
      return { text: "Configurado", kind: "ok", codes: codes(p.hdmi, p.display_port) };
    case "known":
      return { text: p.user ? "Perfil tuyo" : "En la base", kind: "known", codes: codes(p.hdmi, p.display_port) };
    case "none":
      return { text: "Sin perfil", kind: "none", codes: null };
  }
}

export type RowAction = "calibrate" | "recalibrate" | "use" | "export";

/** Los botones de cada monitor: calibrar el que no tiene perfil; usar el de la base. */
export function actions(d: DisplayView): RowAction[] {
  switch (d.profile.kind) {
    case "none":
      return ["calibrate"];
    case "known":
      return d.monitor ? ["use", "calibrate", "export"] : ["calibrate", "export"];
    case "configured":
      return ["recalibrate"];
  }
}

/** Monitores de la configuración que todavía no corresponden a ninguno del bus. */
export function unbound(desk: Desk | null, displays: DisplayView[]): { id: string; label: string }[] {
  const taken = new Set(displays.map((d) => d.monitor).filter((m): m is string => !!m));
  return (desk?.monitors ?? []).filter((m) => !taken.has(m.id)).map((m) => ({ id: m.id, label: m.label }));
}

function capitalize(text: string) {
  return text.charAt(0).toUpperCase() + text.slice(1);
}
