// La sonda de calibración (T-078, T-081): lo que dice la ventana a pantalla completa en el
// monitor que se calibra, la pregunta «¿qué ves?» de la ventana principal y cuánto falta
// para que venza. Pura, para probarla sin DOM.

import type { Input, Machine, ProbeAnswer, ProbeShow, ProbeView } from "./model";

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

/** «la Mac», «la PC». */
export const THE_MACHINE: Record<Machine, string> = { mac: "la Mac", pc: "la PC" };

/** Lo que dice la sonda a pantalla completa. Solo la ve quien ve esta máquina. */
export function probeText(p: ProbeShow): ProbeText {
  const port = PORT_NAME[p.input];
  const who = THE_MACHINE[p.machine];
  switch (p.ask.kind) {
    case "user_switch":
      return {
        title: "Cuando veas esta pantalla, haz clic",
        detail: `Cambia el ${p.monitor} a ${port} desde el menú del monitor: aquí se ve ${who}.`,
        button: "La veo",
      };
    case "candidate":
    case "stalled":
      return {
        title: "¿Ves esta pantalla?",
        detail: `Calibrando el ${p.monitor}: mandé ${hex(p.ask.code)} para ${port} (${who}).`,
        button: "Sí, la veo",
      };
    case "back":
      return {
        title: "¿Ves esta pantalla?",
        detail: `Volví con ${hex(p.ask.code)}: el ${p.monitor} debería mostrar ${who}.`,
        button: "Sí, la veo",
      };
    case "verify":
      return {
        title: "¿Ves esta pantalla?",
        detail: `Verificación del ${p.monitor}: mandé ${hex(p.ask.code)} para ${port} (${who}).`,
        button: "Sí, la veo",
      };
  }
}

export interface Choice {
  label: string;
  answer: ProbeAnswer;
  kind?: "primary" | "danger";
}

export interface QuestionText {
  text: string;
  choices: Choice[];
}

/** Las respuestas de la pregunta: qué máquina se ve, negro, o parar. */
export const CHOICES: Choice[] = [
  { label: "Veo la Mac", answer: { kind: "machine", machine: "mac" } },
  { label: "Veo la PC", answer: { kind: "machine", machine: "pc" } },
  { label: "Negro o sin señal", answer: { kind: "black" } },
  { label: "Parar", answer: { kind: "stop" }, kind: "danger" },
];

/**
 * La pregunta de la ventana principal: «¿qué ves en el monitor?», con un botón por máquina
 * (T-081: con «¿se ve la máquina de HDMI?» se confirmó un código viendo Windows).
 */
export function questionText(p: ProbeShow): QuestionText {
  const port = PORT_NAME[p.input];
  const who = THE_MACHINE[p.machine];
  const what = `¿Qué ves en el ${p.monitor}?`;
  let text: string;
  switch (p.ask.kind) {
    case "user_switch":
      text = `Pasa el ${p.monitor} a ${port} desde su menú para ver ${who}. Cuando cambie, ¿qué ves?`;
      break;
    case "candidate":
      text = `Mandé ${hex(p.ask.code)} para ${port} (${who}). ${what}`;
      break;
    case "back":
      text = `Volví con ${hex(p.ask.code)}: debería verse ${who}. ${what}`;
      break;
    case "verify":
      text = `Verificación: mandé ${hex(p.ask.code)}, debería verse ${who}. ${what}`;
      break;
    case "stalled":
      text = `Nadie contestó tras mandar ${hex(p.ask.code)} dos veces. ¿Sigo? Di qué ves en el ${p.monitor}, o para.`;
      break;
  }
  return { text, choices: CHOICES };
}

/** Segundos enteros que faltan, sin bajar de 0. */
export function secondsLeft(timeoutMs: number, elapsedMs: number): number {
  return Math.max(0, Math.ceil((timeoutMs - elapsedMs) / 1000));
}

export function countdown(seconds: number): string {
  return seconds > 0 ? `Quedan ${seconds} s para contestar.` : "Sin respuesta: se repite el código y después se pregunta si seguir.";
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
    machine: params.get("input") === "dp" ? "pc" : "mac",
    ask: params.get("ask") === "switch" ? { kind: "user_switch" } : { kind: "candidate", code: 0x05 },
    timeout_ms: Number(params.get("timeout") ?? 20000),
    place: params.get("place") === "question" ? { kind: "question" } : { kind: "monitor", screen: null },
  };
}

/** La sonda que dejó la app antes de cargar la página (`tandem_ui::probe::init_script`). */
export function readProbe(injected: unknown): ProbeShow | null {
  if (typeof injected !== "object" || injected === null) return null;
  const v = injected as Partial<ProbeShow>;
  if (v.type !== "show" || typeof v.id !== "number" || typeof v.monitor !== "string" || !v.ask || !v.input || !v.machine) return null;
  if (typeof v.timeout_ms !== "number") return null;
  return v as ProbeShow;
}
