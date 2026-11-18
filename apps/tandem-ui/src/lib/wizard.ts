// El asistente de 6 pasos (DESIGN §12, mockups `F-Asis1`…`F-Asis6`): lo que no depende del
// DOM. Qué pasos hay, qué muestra «Lo que encontró», las tarjetas de permisos, el dibujo y el
// veredicto de la prueba de cruce y el resumen final. Cada dato sale del estado real del
// puesto: nada se supone por sistema (D2).

import { formatMs, type CrossPhase } from "./crossing";
import { LINK_NAME } from "./machines";
import type { Machine, Permission, Side } from "./model";
import type { DeskView, MachineView, PermissionRow } from "./store/types";

export const WIZARD_STEPS = ["Esta máquina", "Tu puesto", "Permisos", "Teclado y mouse", "Acomodo y prueba de cruce", "Listo"];

/** Pasos que se abren desde otras partes de la app (índices de `WIZARD_STEPS`). */
export const STEP_MACHINE = 0;
export const STEP_PAIR = 1;
export const STEP_PERMISSIONS = 2;
export const STEP_KEYBOARD = 3;
export const STEP_ARRANGE = 4;
export const STEP_DONE = 5;

/** El comando que agrega Tandem a Claude Code (`docs/dev/mcp.md`). */
export const MCP_COMMAND = "claude mcp add tandem -- tandem mcp";

export type StepMark = "done" | "cur" | "todo";

/** Hecho (verde), actual (resaltado) o por hacer. */
export function stepMark(index: number, current: number): StepMark {
  return index < current ? "done" : index === current ? "cur" : "todo";
}

/** Un paso más adelante (o atrás) sin salirse de la lista. */
export function clampStep(i: number): number {
  return Math.max(0, Math.min(WIZARD_STEPS.length - 1, i));
}

// ---- Paso 1: lo que encontró ----------------------------------------------------------------

export interface FoundRow {
  id: string;
  /** `ok` encontrado, `off` no disponible ahora, `wait` comprobando. */
  state: "ok" | "off" | "wait";
  label: string;
  note: string;
  /** La nota va en letra monoespaciada (una lista de herramientas). */
  mono: boolean;
}

/** Lo que cada módulo activo dice de sí, en pocas palabras (como el mockup). */
const SHORT: Record<string, string> = { input: "captura e inyección", clipboard: "texto, HTML, imagen", monitors: "canal DDC" };
const LABEL: Record<string, string> = { tunnels: "Cómputo" };
const ORDER = ["input", "clipboard", "tunnels", "monitors"];

/** Los entornos con las herramientas del primero que las anuncia: «zsh · Docker · uv · node». */
export function computeNote(m: MachineView): string {
  const names = m.environments.map((e) => e.name);
  const tools = (m.environments.find((e) => e.kind === "shell")?.detail ?? "")
    .split(" · ")
    .filter((t) => t && !names.some((n) => n.toLowerCase() === t.toLowerCase()));
  return [...names, ...tools].slice(0, 4).join(" · ");
}

const GIB = 1024 ** 3;

/** «macOS 26 · Apple M5 · 24 GB»: el sistema y el equipo según el escaneo (D10). */
export function systemNote(m: MachineView): string {
  const hw = m.hardware;
  const parts = [m.system, hw?.cpu, hw && hw.memory > 0 ? `${Math.round(hw.memory / GIB)} GB` : null].filter((p): p is string => !!p);
  return parts.length > 1 ? parts.join(" · ") : m.detail;
}

export function foundRows(m: MachineView | null): FoundRow[] {
  if (!m) return [];
  const rows: FoundRow[] = [{ id: "system", state: "ok", label: systemNote(m), note: "sistema", mono: false }];
  for (const id of ORDER) {
    const mod = m.modules.find((x) => x.id === id);
    if (!mod) continue;
    const label = LABEL[id] ?? mod.name;
    if (id === "tunnels" && mod.state === "active") rows.push({ id, state: "ok", label, note: computeNote(m), mono: true });
    else if (mod.state === "active") rows.push({ id, state: "ok", label, note: SHORT[id] ?? mod.detail, mono: false });
    else if (id === "monitors" && mod.state === "off") rows.push({ id, state: "off", label, note: "sin canal DDC ahora; otra máquina los controla", mono: false });
    else rows.push({ id, state: "off", label, note: mod.detail, mono: false });
  }
  const admin = m.modules.find((x) => x.id === "tunnels");
  if (admin) rows.push({ id: "service", state: admin.state === "active" ? "ok" : "wait", label: "Servicio de administrador", note: "", mono: false });
  return rows;
}

// ---- Paso 3: permisos -----------------------------------------------------------------------

export interface PermCard {
  id: Permission;
  title: string;
  why: string;
  granted: boolean;
}

// Los textos de `F-Asis3`: se dirigen a las otras máquinas del puesto sin nombrar ningún sistema.
const PERM_TEXT: Record<Permission, { title: string; why: string }> = {
  accessibility: { title: "Accesibilidad", why: "Para escribir y mover el mouse en esta máquina cuando los usas desde otra." },
  input_monitoring: { title: "Monitorización de entrada", why: "Para llevar el teclado y el mouse de esta máquina a otra cuando el cursor cruza el borde." },
  local_network: { title: "Red local", why: "Para encontrar las otras máquinas del puesto y hablar con ellas cifrado." },
};

export function permCards(rows: PermissionRow[]): PermCard[] {
  return rows.map((r) => ({ id: r.id, ...PERM_TEXT[r.id], granted: r.state === "granted" }));
}

export const missingOf = (cards: PermCard[]): number => cards.filter((c) => !c.granted).length;

/** «Faltan 2», «Falta 1» o «Todo concedido». */
export function permFooter(missing: number): string {
  return missing === 0 ? "Todo concedido" : missing === 1 ? "Falta 1" : `Faltan ${missing}`;
}

// ---- Paso 4: teclado y mouse ----------------------------------------------------------------

export interface DeviceRow {
  name: string;
  via?: string;
}

/** Quién ya mostró actividad: una tecla o un movimiento recibidos en esa máquina. */
export type Activity = Record<Machine, boolean>;
export const NO_ACTIVITY: Activity = { a: false, b: false };

export function heard(a: Activity, m: Machine): Activity {
  return a[m] ? a : { ...a, [m]: true };
}

/** Teclados y mouse de una máquina: los que anuncia, o el módulo de entrada si no los lista. */
export function deviceRows(m: MachineView | null): DeviceRow[] {
  if (!m) return [];
  if (m.inputDevices) return m.inputDevices;
  const input = m.modules.find((x) => x.id === "input");
  return input?.state === "active" ? [{ name: "Teclado y mouse" }] : [];
}

// ---- Paso 5: acomodo y prueba de cruce ------------------------------------------------------

/** Las cajas del dibujo, en el lienzo de 290 px de alto de `F-Asis5`: la propia, la otra y el borde. */
export interface Sketch {
  here: string;
  other: string;
  edge: string;
  /** Recorrido del cursor: de dónde sale, por dónde pasa y dónde termina, en % del lienzo (x) o px (y). */
  axis: "x" | "y";
  path: [number, number, number];
  /** Dónde se queda el cursor al cruzar. */
  done: string;
}

export const SKETCH: Record<Side, Sketch> = {
  right: {
    here: "left:12%;top:90px;width:28%;height:130px",
    other: "left:40%;top:40px;width:46%;height:200px",
    edge: "left:calc(40% - 2px);top:60px;width:4px;height:170px",
    axis: "x",
    path: [18, 52, 58],
    done: "left:58%",
  },
  left: {
    here: "left:60%;top:90px;width:28%;height:130px",
    other: "left:14%;top:40px;width:46%;height:200px",
    edge: "left:calc(60% - 2px);top:60px;width:4px;height:170px",
    axis: "x",
    path: [82, 48, 42],
    done: "left:42%",
  },
  down: {
    here: "left:34%;top:20px;width:32%;height:100px",
    other: "left:20%;top:120px;width:60%;height:150px",
    edge: "left:35%;top:118px;width:30%;height:4px",
    axis: "y",
    path: [60, 130, 150],
    done: "top:150px",
  },
  up: {
    here: "left:34%;top:170px;width:32%;height:100px",
    other: "left:20%;top:20px;width:60%;height:150px",
    edge: "left:35%;top:168px;width:30%;height:4px",
    axis: "y",
    path: [220, 150, 130],
    done: "top:130px",
  },
};

const EDGE_WORD: Record<Side, string> = { right: "derecho", left: "izquierdo", up: "superior", down: "inferior" };
const BY_WORD: Record<Side, string> = { right: "la derecha", left: "la izquierda", up: "arriba", down: "abajo" };

export const edgeWord = (s: Side): string => EDGE_WORD[s];

export interface CrossVerdict {
  tone: "ok" | "warn" | "wait";
  text: string;
}

/**
 * Lo que se le dice al usuario según la fase de la prueba. `expected` es el borde del acomodo,
 * `here` el nombre de esta máquina y `peer` el de la otra.
 */
export function crossVerdict(phase: CrossPhase, expected: Side, here: string, peer: string, seconds: number): CrossVerdict {
  switch (phase.kind) {
    case "waiting":
      return { tone: "wait", text: `Esperando el cruce por el borde ${EDGE_WORD[expected]} de ${here}…` };
    case "passed":
      return { tone: "ok", text: `Cruzó a ${peer} por el borde ${EDGE_WORD[expected]}, como dice el acomodo. Vuelta medida: ${formatMs(phase.sample.ms)}.` };
    case "wrong-edge":
      return {
        tone: "warn",
        text: `Cruzó por ${BY_WORD[phase.sample.edge]}, no por ${BY_WORD[expected]}: el acomodo no coincide con tu escritorio. Mueve las pantallas y prueba de nuevo.`,
      };
    case "timeout":
      return { tone: "warn", text: `No cruzó en ${seconds} s. Lleva el mouse hasta el borde ${EDGE_WORD[expected]} de ${here} y prueba de nuevo, o mueve las pantallas.` };
    case "unavailable":
      return { tone: "warn", text: `No se puede probar el cruce todavía: ${phase.reason}.` };
  }
}

/** Dónde queda la otra máquina según adónde se soltó, visto desde el centro de la propia. */
export function dropSide(dx: number, dy: number): Side {
  return Math.abs(dx) >= Math.abs(dy) ? (dx >= 0 ? "right" : "left") : dy >= 0 ? "down" : "up";
}

/** La flecha del teclado mueve la otra máquina hacia ese lado. */
export function arrowSide(key: string): Side | null {
  return key === "ArrowRight" ? "right" : key === "ArrowLeft" ? "left" : key === "ArrowUp" ? "up" : key === "ArrowDown" ? "down" : null;
}

// ---- Paso 6: resumen -------------------------------------------------------------------------

export interface SummaryRow {
  id: string;
  label: string;
  note: string;
  mono: boolean;
  /** Lo que se verificó de verdad; `false` si todavía no. */
  ok: boolean;
}

export interface Facts {
  /** El cruce del paso 5 pasó con el acomodo guardado. */
  crossed: boolean;
}

/** «USB-C · 0.3 ms». */
export function linkNote(d: DeskView): string | null {
  const l = d.link;
  if (!l.kind) return null;
  return l.ms === null ? LINK_NAME[l.kind] : `${LINK_NAME[l.kind]} · ${formatMs(l.ms)}`;
}

export function summaryRows(d: DeskView, facts: Facts): SummaryRow[] {
  const here = d.machines[d.here];
  const peer = d.machines[d.here === "a" ? "b" : "a"];
  const link = linkNote(d);
  const input = here?.modules.find((x) => x.id === "input");
  const mon = d.monitors.find((x) => x.compat.ok && x.connected);
  const tunnels = [here, peer].every((m) => m?.modules.find((x) => x.id === "tunnels")?.state === "active");
  return [
    { id: "link", label: "Enlace", note: link ?? "sin enlace", mono: link !== null, ok: link !== null },
    {
      id: "input",
      label: "Teclado y mouse",
      note: facts.crossed ? "desde cualquier máquina · cruce probado" : "cruce sin probar",
      mono: false,
      ok: input?.state === "active" && facts.crossed,
    },
    {
      id: "monitors",
      label: "Monitores",
      note: mon ? `${mon.name} compatible${mon.owner ? ` · dueño ${d.machines[mon.owner]?.name ?? ""}` : ""}` : "ninguno compatible",
      mono: false,
      ok: !!mon,
    },
    {
      id: "tunnels",
      label: "Cómputo remoto",
      note: tunnels ? "MCP listo para Claude Code en las dos" : "falta el cómputo remoto en alguna",
      mono: false,
      ok: tunnels,
    },
  ];
}

/** La primera fila del resumen: «MacBook Pro y PC-Escritorio en «Mi puesto»». */
export function deskTitle(d: DeskView): { here: string; peer: string | null; desk: string } {
  const peerId: Machine = d.here === "a" ? "b" : "a";
  const peer = d.pair.peer.kind === "paired" ? (d.machines[peerId]?.name ?? d.pair.peer.name) : null;
  return { here: d.machines[d.here]?.name ?? "Esta máquina", peer, desk: d.deskName };
}
