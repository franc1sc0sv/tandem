// El asistente de 6 pasos (DESIGN §12, mockups `F-Setup1`…`F-Setup6`): lo que no depende del
// DOM. Qué pasos hay, qué muestra «What it found», las tarjetas de permisos, el dibujo y el
// verdict de la prueba de crossing y el resumen final. Cada dato sale del estado real del
// desk: nada se supone por sistema (D2).

import { formatMs, type CrossPhase } from "./crossing";
import { LINK_NAME } from "./machines";
import type { AdminView, Machine, Permission, Side } from "./model";
import type { DeskView, MachineView, PermissionRow } from "./store/types";

export const WIZARD_STEPS = ["This machine", "Your desk", "Permissions", "Keyboard and mouse", "Arrangement and crossing test", "Done"];

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
  /** `ok` encontrado, `off` unavailable ahora, `wait` comprobando. */
  state: "ok" | "off" | "wait";
  label: string;
  note: string;
  /** La nota va en letra monoespaciada (una lista de herramientas). */
  mono: boolean;
  /** Solo la fila del servicio: hay botón «Instalar». */
  install?: boolean;
}

/** Lo que cada módulo activo dice de sí, en pocas palabras (como el mockup). */
const SHORT: Record<string, string> = { input: "capture and injection", clipboard: "text, HTML, image", monitors: "DDC channel" };
const LABEL: Record<string, string> = { tunnels: "Compute" };
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

/**
 * La fila del servicio de administrador según lo que reporta la app (`off` / `pending` / `ok`).
 * Sin lectura todavía se usa el módulo del agente, pero nunca queda girando: sin dato, `off`.
 */
export function serviceRow(admin: AdminView | null, moduleActive: boolean): FoundRow {
  const base = { id: "service", label: "Administrator service", mono: false };
  if (!admin) return { ...base, state: moduleActive ? "ok" : "off", note: "" };
  const state = admin.state === "ok" ? "ok" : admin.state === "pending" ? "wait" : "off";
  return { ...base, state, note: admin.canInstall ? "" : admin.note, install: admin.canInstall && admin.state !== "ok" };
}

export function foundRows(m: MachineView | null, admin: AdminView | null = null): FoundRow[] {
  if (!m) return [];
  const rows: FoundRow[] = [{ id: "system", state: "ok", label: systemNote(m), note: "system", mono: false }];
  for (const id of ORDER) {
    const mod = m.modules.find((x) => x.id === id);
    if (!mod) continue;
    const label = LABEL[id] ?? mod.name;
    // El cómputo se dibuja con los entornos que hay, aunque falte el servicio de administrador.
    if (id === "tunnels" && (mod.state === "active" || m.environments.length > 0)) rows.push({ id, state: "ok", label, note: computeNote(m), mono: true });
    else if (mod.state === "active") rows.push({ id, state: "ok", label, note: SHORT[id] ?? mod.detail, mono: false });
    else if (id === "monitors" && mod.state === "off") rows.push({ id, state: "off", label, note: "no DDC channel right now; another machine controls them", mono: false });
    else rows.push({ id, state: "off", label, note: mod.detail, mono: false });
  }
  const tunnels = m.modules.find((x) => x.id === "tunnels");
  // La app es quien sabe del servicio: la fila sale aunque el agente no anuncie el módulo.
  if (tunnels || admin) rows.push(serviceRow(admin, tunnels?.state === "active"));
  return rows;
}

// ---- Paso 3: permisos -----------------------------------------------------------------------

export interface PermCard {
  id: Permission;
  title: string;
  why: string;
  /** Concedido y comprobado por el agente. */
  granted: boolean;
  /** Falta de verdad. Lo que el agente no puede comprobar (Red local) ni cuenta como faltante ni como concedido. */
  missing: boolean;
}

// Los textos de `F-Setup3`: se dirigen a las otras machines del desk sin nombrar ningún sistema.
const PERM_TEXT: Record<Permission, { title: string; why: string }> = {
  accessibility: { title: "Accessibility", why: "To type and move the mouse on this machine when you use them from another one." },
  input_monitoring: { title: "Input Monitoring", why: "To carry this machine’s keyboard and mouse to another one when the cursor crosses the edge." },
  local_network: { title: "Local Network", why: "To find the other machines of the desk and talk to them encrypted." },
};

export function permCards(rows: PermissionRow[]): PermCard[] {
  return rows.map((r) => ({ id: r.id, ...PERM_TEXT[r.id], granted: r.state === "granted", missing: r.state === "missing" }));
}

export const missingOf = (cards: PermCard[]): number => cards.filter((c) => c.missing).length;

/** «Missing 2» or «All granted». */
export function permFooter(missing: number): string {
  return missing === 0 ? "All granted" : `Missing ${missing}`;
}

// ---- Paso 4: teclado y mouse ----------------------------------------------------------------

export interface DeviceRow {
  name: string;
  via?: string;
}

/** Quién ya mostró actividad: una tecla o un movimiento recibidos en esa machine. */
export type Activity = Record<Machine, boolean>;
export const NO_ACTIVITY: Activity = { a: false, b: false };

export function heard(a: Activity, m: Machine): Activity {
  return a[m] ? a : { ...a, [m]: true };
}

/** Teclados y mouse de una machine: solo los que el agente anuncia; lo supuesto no cuenta. */
export function deviceRows(m: MachineView | null): DeviceRow[] {
  return m?.inputDevices ?? [];
}

/** Lo que el agente anuncia del hub: sin `true` no hay teclado ni mouse que mostrar. */
export function devicesOfHub(hub: boolean | null | undefined): DeviceRow[] {
  return hub === true ? [{ name: "Keyboard and mouse", via: "through the hub" }] : [];
}

/** El cómputo funciona: el módulo de tunnels no está apagado y hay un entorno listo. */
export function computeWorks(m: MachineView | null | undefined): boolean {
  if (!m) return false;
  // Un módulo que el agente da por unavailable tampoco cuenta como cómputo remoto (T-158).
  const state = m.modules.find((x) => x.id === "tunnels")?.state;
  return state !== "off" && state !== "unavailable" && m.environments.some((e) => e.ready);
}

// ---- Paso 5: arrangement y prueba de crossing ------------------------------------------------------

/** Las cajas del dibujo, en el lienzo de 290 px de alto de `F-Setup5`: la propia, la otra y el borde. */
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

const EDGE_WORD: Record<Side, string> = { right: "right", left: "left", up: "top", down: "bottom" };
const BY_WORD: Record<Side, string> = { right: "the right", left: "the left", up: "the top", down: "the bottom" };

export const edgeWord = (s: Side): string => EDGE_WORD[s];

export interface CrossVerdict {
  tone: "ok" | "warn" | "wait";
  text: string;
}

/**
 * Lo que se le dice al usuario según la fase de la prueba. `expected` es el borde del arrangement,
 * `here` el nombre de this machine y `peer` el de la otra.
 */
export function crossVerdict(phase: CrossPhase, expected: Side, here: string, peer: string, seconds: number): CrossVerdict {
  switch (phase.kind) {
    case "waiting":
      return { tone: "wait", text: `Waiting for the crossing at the ${EDGE_WORD[expected]} edge of ${here}…` };
    case "passed":
      return { tone: "ok", text: `Crossed to ${peer} through the ${EDGE_WORD[expected]} edge, as the arrangement says.${phase.sample.ms === null ? "" : ` Round trip measured: ${formatMs(phase.sample.ms)}.`}` };
    case "wrong-edge":
      return {
        tone: "warn",
        text: `Crossed at ${BY_WORD[phase.sample.edge]}, not ${BY_WORD[expected]}: the arrangement does not match your desk. Move the screens and test again.`,
      };
    case "timeout":
      return { tone: "warn", text: `Did not cross in ${seconds} s. Move the mouse to the ${EDGE_WORD[expected]} edge of ${here} and test again, or move the screens.` };
    case "unavailable":
      return { tone: "warn", text: `The crossing cannot be tested yet: ${phase.reason}.` };
  }
}

/** Dónde queda the other machine según adónde se soltó, visto desde el centro de la propia. */
export function dropSide(dx: number, dy: number): Side {
  return Math.abs(dx) >= Math.abs(dy) ? (dx >= 0 ? "right" : "left") : dy >= 0 ? "down" : "up";
}

/** La flecha del teclado mueve the other machine hacia ese lado. */
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
  /** El crossing del paso 5 pasó con el arrangement guardado. */
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
  const tunnels = [here, peer].every(computeWorks);
  return [
    { id: "link", label: "Link", note: link ?? "no link", mono: link !== null, ok: link !== null },
    {
      id: "input",
      label: "Keyboard and mouse",
      note: facts.crossed ? "from any machine · crossing tested" : "crossing not tested",
      mono: false,
      ok: input?.state === "active" && facts.crossed,
    },
    {
      id: "monitors",
      label: "Monitors",
      note: mon ? `${d.layout.screens.find((x) => x.id === mon.id)?.size ?? mon.name} compatible${mon.owner ? ` · owner ${d.machines[mon.owner]?.name ?? ""}` : ""}` : "none supported",
      mono: false,
      ok: !!mon,
    },
    {
      id: "tunnels",
      label: "Remote compute",
      note: tunnels ? "MCP ready for Claude Code on both" : "remote compute does not respond on one of them",
      mono: false,
      ok: tunnels,
    },
  ];
}

/** La primera fila del resumen: «MacBook Pro y PC-Escritorio en «My desk»». */
export function deskTitle(d: DeskView): { here: string; peer: string | null; desk: string } {
  const peerId: Machine = d.here === "a" ? "b" : "a";
  const peer = d.pair.peer.kind === "paired" ? (d.machines[peerId]?.name ?? d.pair.peer.name) : null;
  return { here: d.machines[d.here]?.name ?? "This machine", peer, desk: d.deskName };
}
