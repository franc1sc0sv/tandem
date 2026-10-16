// El estado inicial real del puesto (T-085). Los monitores no dicen su entrada y cada
// sistema ve al monitor fantasma como activo: lo que muestra cada uno lo confirma
// «Aplicar modo» (Tandem manda todas las entradas) o el usuario. Puro, para probarlo sin DOM.

import type { Caps, Desk, DeskState, Doubt, Machine, MonitorConfig, PeerView } from "./model";
import { MACHINE_NAME, capsOf, other } from "./scene";

/** Lo que el usuario dice que muestra un monitor: una máquina, o nada (en negro). */
export type Shown = Machine | null;

export function doubtOf(state: DeskState, id: string): Doubt | null {
  return state.doubt?.[id] ?? null;
}

/** Hay que preguntar o aplicar: lo de este monitor no se sabe (o es de otro arranque). */
export function isUnsure(state: DeskState, id: string): boolean {
  const d = doubtOf(state, id);
  return d === "unknown" || d === "stale";
}

/** Tandem sabe lo que muestra cada monitor del puesto (uno en negro cuenta como sabido). */
export function isConfirmed(desk: Desk, state: DeskState): boolean {
  return !desk.monitors.some((m) => isUnsure(state, m.id));
}

/** Las máquinas que pueden cambiarle la entrada a `id`; sin anuncio de la otra, se supone que ella puede lo que esta no. */
export function switchersOf(caps: Caps, id: string): Machine[] {
  const can = (c: Caps["here"]) => c.monitors.some((m) => m.switch && m.monitor === id);
  const who: Machine[] = [];
  if (can(caps.here)) who.push(caps.here.machine);
  const peer = other(caps.here.machine);
  if (caps.peer ? can(caps.peer) : who.length === 0) who.push(peer);
  return who;
}

export interface ApplyReady {
  ready: boolean;
  /** Por qué no, en una línea; `null` si se puede. */
  why: string | null;
}

/**
 * «Aplicar modo» necesita, para cada monitor, una máquina que pueda cambiarle la entrada
 * (hoy la PC, por DDC) encendida y, si no es esta, enlazada. Si no, se ofrece responder.
 */
export function applyReady(desk: Desk, state: DeskState, caps: Caps, agent: Machine, peer: PeerView): ApplyReady {
  const linked = peer.kind === "paired" && peer.link !== null;
  const missing = new Set<Machine>();
  for (const m of desk.monitors) {
    const who = switchersOf(caps, m.id);
    if (who.length === 0) return { ready: false, why: `nadie puede cambiarle la entrada al ${m.label}` };
    const ok = who.some((w) => state.on[w] && (w === agent || linked));
    if (!ok) who.forEach((w) => missing.add(w));
  }
  if (missing.size === 0) return { ready: true, why: null };
  const names = [...missing].map((w) => `la ${MACHINE_NAME[w]}`).join(" o ");
  return { ready: false, why: `para aplicar un modo, ${names} (la que cambia las entradas) tiene que estar encendida y enlazada` };
}

export interface Hint {
  shows: Shown;
  why: string;
}

/** Las máquinas a las que va cableado el monitor: lo único que puede mostrar. */
export function choicesOf(m: MonitorConfig): Machine[] {
  return (["mac", "pc"] as Machine[]).filter((w) => m.wiring[w] !== undefined);
}

/**
 * Una pista para preseleccionar la respuesta; el usuario confirma. Una máquina apagada no
 * se ve; si solo una de las dos detectó el monitor al arrancar, es la que lo muestra; si no,
 * lo último que se supo (de un arranque anterior). Sin nada de eso, ninguna.
 */
export function hintFor(m: MonitorConfig, state: DeskState, caps: Caps): Hint | null {
  const wired = choicesOf(m);
  if (wired.length === 1) return { shows: wired[0], why: `solo va cableado a la ${MACHINE_NAME[wired[0]]}` };
  const on = wired.filter((w) => state.on[w]);
  if (on.length === 1) return { shows: on[0], why: `la ${MACHINE_NAME[other(on[0])]} está apagada` };
  const seen = wired.filter((w) => capsOf(caps, w)?.monitors.some((x) => x.monitor === m.id && x.seen));
  const announced = wired.filter((w) => capsOf(caps, w) !== null);
  if (seen.length === 1 && announced.length === 2) return { shows: seen[0], why: `solo la ${MACHINE_NAME[seen[0]]} lo detecta` };
  if (doubtOf(state, m.id) === "stale" && state.owner[m.id]) {
    return { shows: state.owner[m.id], why: "lo último que se supo" };
  }
  return null;
}

export interface Question {
  id: string;
  label: string;
  choices: Machine[];
  hint: Hint | null;
}

/** «¿Qué muestra ahora?» para cada monitor sin confirmar, en el orden del escritorio. */
export function questions(desk: Desk, state: DeskState, caps: Caps): Question[] {
  return desk.monitors
    .filter((m) => isUnsure(state, m.id))
    .map((m) => ({ id: m.id, label: m.label, choices: choicesOf(m), hint: hintFor(m, state, caps) }));
}

/** Las respuestas de partida: la pista de cada pregunta, si la hay. */
export function initialAnswers(qs: Question[]): Record<string, Shown> {
  return Object.fromEntries(qs.flatMap((q) => (q.hint ? [[q.id, q.hint.shows] as const] : [])));
}

/** Se puede mandar cuando cada pregunta tiene respuesta. */
export function complete(qs: Question[], answers: Record<string, Shown | undefined>): boolean {
  return qs.length > 0 && qs.every((q) => answers[q.id] !== undefined);
}

/** El texto de un monitor sin confirmar en la escena. */
export function doubtNote(state: DeskState, id: string): string | null {
  const d = doubtOf(state, id);
  if (d === "blank") return "en negro";
  if (d === "unknown" || d === "stale") return "sin confirmar";
  return null;
}

/** Las opciones de «en realidad muestra…» para corregir un monitor desde la escena. */
export function corrections(m: MonitorConfig, state: DeskState): { shows: Shown; label: string; current: boolean }[] {
  const owner = state.owner[m.id] ?? "mac";
  const blank = doubtOf(state, m.id) === "blank";
  const known = !isUnsure(state, m.id);
  return [
    ...choicesOf(m).map((w) => ({ shows: w as Shown, label: `la ${MACHINE_NAME[w]}`, current: known && !blank && owner === w })),
    { shows: null, label: "nada (negro)", current: blank },
  ];
}
