// La sonda de calibración (T-078): lo que dice la ventana a pantalla completa en el monitor
// que se calibra, la pregunta de respaldo de la ventana principal y cuánto falta para que
// venza. Pura, para probarla sin DOM.

import type { Input, ProbeShow, ProbeView } from "./model";

export const PORT_NAME: Record<Input, string> = { hdmi: "HDMI", displayport: "DisplayPort" };

export interface ProbeText {
  title: string;
  detail: string;
  button: string;
}

/** `0x0F`: como lo escriben los perfiles y la investigación. */
export function hex(code: number): string {
  return `0x${code.toString(16).toUpperCase().padStart(2, "0")}`;
}

/** Lo que dice la sonda a pantalla completa. */
export function probeText(p: ProbeShow): ProbeText {
  const port = PORT_NAME[p.input];
  if (p.ask.kind === "user_switch") {
    return {
      title: "Cuando veas esta pantalla, haz clic",
      detail: `Cambia el ${p.monitor} a ${port} desde el menú del monitor.`,
      button: "La veo",
    };
  }
  return {
    title: "¿Ves esta pantalla?",
    detail: `Calibrando el ${p.monitor}: entrada ${port}, código ${hex(p.ask.code)}.`,
    button: "Sí, la veo",
  };
}

export interface QuestionText {
  text: string;
  yes: string;
  no: string;
}

/** La pregunta de respaldo (sin la otra máquina), en la ventana principal. */
export function questionText(p: ProbeShow): QuestionText {
  const port = PORT_NAME[p.input];
  if (p.ask.kind === "user_switch") {
    return { text: `Cambia el ${p.monitor} a ${port} desde el menú del monitor. ¿Ya cambió?`, yes: "Ya cambió", no: "No puedo" };
  }
  return { text: `¿El ${p.monitor} pasó a ${port}? (código ${hex(p.ask.code)})`, yes: "Sí, cambió", no: "No" };
}

/** Segundos enteros que faltan, sin bajar de 0. */
export function secondsLeft(timeoutMs: number, elapsedMs: number): number {
  return Math.max(0, Math.ceil((timeoutMs - elapsedMs) / 1000));
}

export function countdown(seconds: number): string {
  return seconds > 0 ? `Si no la ves, no hagas nada: se cierra sola en ${seconds} s.` : "Sin respuesta: cuenta como no vista.";
}

/** La pregunta pendiente de la ventana principal tras un aviso de sonda. */
export function nextQuestion(current: ProbeShow | null, ev: ProbeView): ProbeShow | null {
  if (ev.type === "show") return ev.place.kind === "question" ? ev : current;
  return current && current.id === ev.id ? null : current;
}

/** Una sonda de muestra para el modo demo (`probe.html?demo`, `&ask=switch`, `&input=dp`). */
export function demoProbe(search: string): ProbeShow {
  const params = new URLSearchParams(search);
  return {
    type: "show",
    id: 1,
    monitor: params.get("monitor") ?? "Odyssey G5",
    input: params.get("input") === "dp" ? "displayport" : "hdmi",
    ask: params.get("ask") === "switch" ? { kind: "user_switch" } : { kind: "candidate", code: 0x05 },
    timeout_ms: Number(params.get("timeout") ?? 8000),
    place: params.get("place") === "question" ? { kind: "question" } : { kind: "monitor", screen: null },
  };
}

/** La sonda que dejó la app antes de cargar la página (`tandem_ui::probe::init_script`). */
export function readProbe(injected: unknown): ProbeShow | null {
  if (typeof injected !== "object" || injected === null) return null;
  const v = injected as Partial<ProbeShow>;
  if (v.type !== "show" || typeof v.id !== "number" || typeof v.monitor !== "string" || !v.ask || !v.input) return null;
  if (typeof v.timeout_ms !== "number") return null;
  return v as ProbeShow;
}
