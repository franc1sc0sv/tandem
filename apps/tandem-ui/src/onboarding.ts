// El asistente de configuración inicial (T-087): un paso por pantalla y cada paso verificado
// antes de pasar al siguiente. Aquí, lo que no depende del DOM: el estado del asistente, lo
// que se muestra en cada paso (`viewOf`) y las reglas de cuándo se puede avanzar. Los efectos
// (pedirle cosas al agente, escuchar el teclado) viven en `onboarding-ui.ts`.

import { isConfirmed } from "./confirm";
import type {
  Action,
  CapsMonitor,
  DeskState,
  Input,
  Link,
  Machine,
  MonitorCandidate,
  MonitorConfig,
  MonitorScan,
  NewMonitor,
  Snapshot,
} from "./model";
import { LINK_NAME, MACHINE_NAME, PORT, other, type ModeKey } from "./scene";

export type StepId = "machine" | "pair" | "keyboard" | "monitors" | "verify" | "initial" | "summary";

export const STEPS: { id: StepId; title: string }[] = [
  { id: "machine", title: "Esta máquina" },
  { id: "pair", title: "La otra máquina" },
  { id: "keyboard", title: "Teclado y mouse" },
  { id: "monitors", title: "Monitores" },
  { id: "verify", title: "Probar cada monitor" },
  { id: "initial", title: "Estado inicial" },
  { id: "summary", title: "Resumen" },
];

const INPUT_NAME: Record<Input, string> = { hdmi: "HDMI", displayport: "DisplayPort" };
const MODE_NAME: Record<ModeKey, string> = { mac: "Solo Mac", shared: "Compartido", pc: "Solo Windows" };

// ---------------------------------------------------------------------------------------
// Estado

export interface PairFacts {
  /** El último ping al otro agente, con el enlace por el que fue. */
  ping: { link: Link; millis: number } | null;
  pingError: string | null;
  /** «Seguir solo por ahora». */
  skipped: boolean;
}

export interface KeyFacts {
  /** Las dos máquinas están escuchando. */
  armed: boolean;
  /** La máquina que recibió la tecla. */
  heard: Machine | null;
  /** Lo que el usuario confirmó (o eligió a mano). */
  confirmed: Machine | null;
  /** «Dejar sin definir»: nunca se supone la Mac. */
  undefinedOk: boolean;
}

export interface MonitorChoice {
  add: boolean;
  /** ¿Son el mismo monitor? `null`: sin responder. */
  linked: boolean | null;
  wiring: Partial<Record<Machine, Input>>;
}

export interface MonitorsFacts {
  scan: MonitorScan | null;
  scanning: boolean;
  error: string | null;
  choices: Record<string, Partial<MonitorChoice>>;
  adding: boolean;
  /** Se agregaron al puesto. */
  added: boolean;
  /** «Seguir sin monitores». */
  skipped: boolean;
}

export type VerifyPhase =
  | { kind: "idle" }
  /** Pasando el monitor a `to`. */
  | { kind: "moving"; to: Machine }
  /** ¿Se ve `machine`? */
  | { kind: "ask"; machine: Machine }
  /** De vuelta a donde estaba; `failed` si algo salió mal antes y ya solo se vuelve. */
  | { kind: "returning"; failed: string | null; silent?: boolean }
  | { kind: "ask_home" }
  | { kind: "ok" }
  | { kind: "manual"; reason: string };

export interface VerifyEntry {
  /** Donde estaba el monitor al empezar; vuelve ahí siempre. */
  home: Machine;
  /** `home` es un relleno (el monitor se marcó manual sin saber dónde estaba): no cuenta como dicho. */
  unknownHome?: boolean;
  phase: VerifyPhase;
}

export interface InitialFacts {
  mode: ModeKey | null;
  applying: boolean;
  applied: boolean;
  looksRight: boolean | null;
}

export interface Onboarding {
  step: StepId;
  pair: PairFacts;
  keyboard: KeyFacts;
  monitors: MonitorsFacts;
  verify: Record<string, VerifyEntry>;
  initial: InitialFacts;
  /** Un mensaje del último pedido que el agente rechazó. */
  error: string | null;
}

export function begin(step: StepId = "machine"): Onboarding {
  return {
    step,
    pair: { ping: null, pingError: null, skipped: false },
    keyboard: { armed: false, heard: null, confirmed: null, undefinedOk: false },
    monitors: { scan: null, scanning: false, error: null, choices: {}, adding: false, added: false, skipped: false },
    verify: {},
    initial: { mode: null, applying: false, applied: false, looksRight: null },
    error: null,
  };
}

/** Si el asistente debe abrirse solo: el puesto está vacío (instalación limpia o restablecido). */
export function shouldOpen(snap: Snapshot): boolean {
  return snap.kind === "connected" && snap.desk.monitors.length === 0 && snap.hidden.length === 0;
}

// ---------------------------------------------------------------------------------------
// Qué se muestra

export type CheckState = "ok" | "wait" | "fail" | "idle";

export interface Check {
  state: CheckState;
  label: string;
  detail?: string;
}

export interface Btn {
  id: string;
  label: string;
  kind?: "primary" | "danger";
  enabled: boolean;
  /** Por qué está apagado, o qué hace. */
  hint?: string;
}

export interface Option {
  id: string;
  label: string;
  selected: boolean;
}

export interface Question {
  text: string;
  options: Option[];
}

/** Una fila de monitor (pasos 4 y 5). */
export interface Row {
  key: string;
  title: string;
  subtitle: string;
  checks: Check[];
  questions: Question[];
  buttons: Btn[];
}

export interface StepView {
  id: StepId;
  title: string;
  lead: string;
  checks: Check[];
  rows: Row[];
  /** Un número grande para mostrar (el código de emparejamiento). */
  code: string | null;
  buttons: Btn[];
  /** El paso quedó verificado: se puede pasar al siguiente. */
  verified: boolean;
  /** Por qué todavía no, en una línea. */
  blocker: string | null;
}

const empty = (id: StepId, lead: string): StepView => ({
  id,
  title: STEPS.find((s) => s.id === id)?.title ?? "",
  lead,
  checks: [],
  rows: [],
  code: null,
  buttons: [],
  verified: false,
  blocker: null,
});

/** El nombre de una identidad de monitor: `SAM711A (LC34G55T)`. */
function identity(m: CapsMonitor | null): string {
  if (!m) return "no lo ve";
  const model = m.model && m.model !== m.pnp_id ? ` (${m.model})` : "";
  return `${m.pnp_id ?? "sin id"}${model}`;
}

export function candidateKey(c: MonitorCandidate): string {
  return c.pnp_ids.length ? c.pnp_ids.join("+") : c.label;
}

/** Lo que vale para un monitor propuesto: lo elegido, y si no, lo que el agente deduce. */
export function choiceOf(o: Onboarding, c: MonitorCandidate): MonitorChoice {
  const picked = o.monitors.choices[candidateKey(c)] ?? {};
  const both = c.here !== null && c.peer !== null;
  return {
    add: picked.add ?? c.in_desk === null,
    // Lo detectado (mismo serie, cable por la topología) es solo una sugerencia: hace
    // falta una pulsación explícita en cada pregunta. Con un solo lado no hay vínculo que
    // confirmar.
    linked: picked.linked ?? (!both ? true : null),
    wiring: { ...picked.wiring },
  };
}

/** Las máquinas que ven el monitor (las que hay que cablear). */
function seenBy(c: MonitorCandidate, agent: Machine): Machine[] {
  return [c.here ? agent : null, c.peer ? other(agent) : null].filter((m): m is Machine => m !== null);
}

/** ¿Está lista una fila para agregarse? Con vínculo confirmado y el cableado de cada máquina que lo ve. */
export function candidateReady(o: Onboarding, c: MonitorCandidate, agent: Machine): boolean {
  const ch = choiceOf(o, c);
  if (ch.linked === null) return false;
  const machines = ch.linked ? seenBy(c, agent) : [];
  if (!ch.linked) return seenBy(c, agent).every((m) => ch.wiring[m] !== undefined);
  const inputs = machines.map((m) => ch.wiring[m]);
  if (inputs.some((i) => i === undefined)) return false;
  // Dos máquinas no pueden ir a la misma entrada.
  return new Set(inputs).size === inputs.length;
}

/** El motivo del primer monitor elegido que no está listo, con su nombre. */
function blockedReason(o: Onboarding, pending: MonitorCandidate[], agent: Machine): string {
  for (const cand of pending) {
    const why = candidateBlocker(o, cand, agent);
    if (why) return `${cand.label}: ${why}`;
  }
  return "faltan respuestas";
}

/** Por qué una fila no está lista para agregarse, con el motivo concreto; `null` si lo está. */
export function candidateBlocker(o: Onboarding, c: MonitorCandidate, agent: Machine): string | null {
  const ch = choiceOf(o, c);
  if (ch.linked === null) return "falta confirmar si el monitor de la PC y el de la Mac son el mismo";
  const machines = seenBy(c, agent);
  const missing = machines.filter((m) => ch.wiring[m] === undefined);
  if (missing.length) return `falta confirmar por qué cable va a la ${missing.map((m) => MACHINE_NAME[m]).join(" y a la ")}`;
  if (ch.linked) {
    const inputs = machines.map((m) => ch.wiring[m]!);
    if (new Set(inputs).size !== inputs.length) {
      return `la ${machines.map((m) => MACHINE_NAME[m]).join(" y la ")} no pueden ir al mismo cable (${INPUT_NAME[inputs[0]!]} en las dos): una va por DisplayPort y la otra por HDMI`;
    }
  }
  return null;
}

/**
 * Lo que se agrega al puesto: un monitor por cada fila marcada; si el usuario dijo que no
 * son el mismo, cada identidad queda como su propio monitor.
 */
export function newMonitors(o: Onboarding, agent: Machine): NewMonitor[] {
  const out: NewMonitor[] = [];
  for (const c of o.monitors.scan?.candidates ?? []) {
    const ch = choiceOf(o, c);
    if (!ch.add || c.in_desk !== null || !candidateReady(o, c, agent)) continue;
    if (ch.linked) {
      out.push({ label: c.label, pnp_ids: c.pnp_ids, wiring: wiringFor(c, ch, agent, [agent, other(agent)]) });
      continue;
    }
    // Dos monitores distintos que casualmente se parecían: uno por cada máquina que lo ve.
    for (const [who, side] of [
      [agent, c.here],
      [other(agent), c.peer],
    ] as const) {
      if (!side) continue;
      out.push({ label: side.name, pnp_ids: side.pnp_id ? [side.pnp_id] : [], wiring: wiringFor(c, ch, agent, [who]) });
    }
  }
  return out;
}

function wiringFor(c: MonitorCandidate, ch: MonitorChoice, agent: Machine, only: Machine[]): Partial<Record<Machine, Input>> {
  const seen = new Set(seenBy(c, agent));
  const out: Partial<Record<Machine, Input>> = {};
  for (const m of only) {
    const i = ch.wiring[m];
    if (seen.has(m) && i) out[m] = i;
  }
  return out;
}

// ---- Verificación de cada monitor (paso 5)

export type VerifyEvent =
  | { type: "start" }
  | { type: "moved"; ok: boolean; reason?: string }
  | { type: "answer"; yes: boolean }
  /** Nadie contestó a tiempo (o se cerró el asistente): cuenta como «no» y el monitor vuelve. */
  | { type: "timeout" };

/** `move`: pasar el monitor a `to` (o devolverlo, con `to === back`); `back` es donde estaba. */
export type VerifyEffect = { kind: "move"; monitor: string; to: Machine; back: Machine } | { kind: "mark_manual"; monitor: string; discard: boolean };

/**
 * Dónde está ahora el monitor: lo que el usuario dijo en el paso 5, o el estado si ya está
 * confirmado. Con el estado sin confirmar y sin respuesta, `null`: no se supone.
 */
export function homeOf(o: Onboarding, state: DeskState, monitor: string): Machine | null {
  const entry = o.verify[monitor];
  if (entry && !entry.unknownHome) return entry.home;
  const owner = state.owner[monitor];
  return owner && !state.doubt[monitor] ? owner : null;
}

/** Un monitor sin entrada propia a donde ir (cableado a una sola máquina) no se puede probar. */
export function canRoundTrip(m: MonitorConfig, home: Machine): boolean {
  return m.wiring[other(home)] !== undefined && m.wiring[home] !== undefined;
}

/**
 * La ida y vuelta real de un monitor: pasa a la otra máquina, pregunta «¿ves la X?», vuelve a
 * donde estaba y pregunta «¿ves la Y?». Si algo falla o el usuario dice que no, vuelve igual
 * (vuelta automática) y el monitor queda en modo manual asistido.
 */
export function verifyStep(monitor: string, entry: VerifyEntry, e: VerifyEvent): { next: VerifyEntry; effects: VerifyEffect[] } {
  const { home, phase } = entry;
  const stay = { next: entry, effects: [] as VerifyEffect[] };
  // Solo lo que el usuario dice haber visto (o no) habla contra los códigos del perfil; un fallo
  // al pasar el monitor o al volver (tiempos agotados, enlace) no prueba nada contra ellos.
  const manual = (reason: string, discard = false): { next: VerifyEntry; effects: VerifyEffect[] } => ({
    next: { home, phase: { kind: "manual", reason } },
    effects: [{ kind: "mark_manual", monitor, discard }],
  });
  switch (e.type) {
    case "start":
      if (phase.kind !== "idle" && phase.kind !== "manual") return stay;
      return { next: { home, phase: { kind: "moving", to: other(home) } }, effects: [{ kind: "move", monitor, to: other(home), back: home }] };
    case "moved":
      if (phase.kind === "moving" && phase.to === other(home)) {
        // No pasó: sigue donde estaba, así que no hay nada que volver.
        if (!e.ok) return manual(e.reason ?? `no se pudo pasar a la ${MACHINE_NAME[other(home)]}`);
        return { next: { home, phase: { kind: "ask", machine: other(home) } }, effects: [] };
      }
      if (phase.kind === "returning") {
        // Si la vuelta también falla, el monitor quizá no volvió: se dice junto al motivo.
        // `failed` solo lo pone un «no» del usuario: eso sí habla contra los códigos.
        if (phase.failed) return manual(e.ok ? phase.failed : `${phase.failed}; además ${e.reason ?? "no se pudo volver"}: puede que el monitor no haya vuelto`, !phase.silent);
        if (!e.ok) return manual(e.reason ?? `no se pudo volver a la ${MACHINE_NAME[home]}`);
        return { next: { home, phase: { kind: "ask_home" } }, effects: [] };
      }
      return stay;
    case "timeout":
      // Sin contestar no es un «no»: no habla contra los códigos del perfil. Vuelve y queda manual.
      if (phase.kind === "ask") {
        return {
          next: { home, phase: { kind: "returning", failed: `sin respuesta a «¿ves la ${MACHINE_NAME[phase.machine]}?»`, silent: true } },
          effects: [{ kind: "move", monitor, to: home, back: home }],
        };
      }
      if (phase.kind === "ask_home") return manual(`sin respuesta a «¿ves la ${MACHINE_NAME[home]}?» al volver`);
      return stay;
    case "answer":
      if (phase.kind === "ask") {
        // Vuelve a donde estaba en cualquier caso; si dijo que no, queda dicho por qué.
        return {
          next: { home, phase: { kind: "returning", failed: e.yes ? null : `no se vio la ${MACHINE_NAME[phase.machine]}` } },
          effects: [{ kind: "move", monitor, to: home, back: home }],
        };
      }
      if (phase.kind === "ask_home") {
        return e.yes ? { next: { home, phase: { kind: "ok" } }, effects: [] } : manual(`no se vio la ${MACHINE_NAME[home]} al volver`, true);
      }
      return stay;
  }
}

/** Un monitor cableado a una sola máquina no tiene a dónde ir: no hay ida y vuelta que probar. */
export function soloWired(m: MonitorConfig, entry: VerifyEntry | undefined): boolean {
  const wired = (["mac", "pc"] as Machine[]).filter((w) => m.wiring[w] !== undefined);
  return wired.length < 2 && !verifyDone(entry);
}

export function verifyDone(entry: VerifyEntry | undefined): boolean {
  return entry?.phase.kind === "ok" || entry?.phase.kind === "manual";
}

function verifyChecks(m: MonitorConfig, entry: VerifyEntry | undefined): Check[] {
  const checks: Check[] = [];
  const codes = m.ddc ? `DP 0x${m.ddc.display_port.toString(16).toUpperCase().padStart(2, "0")} · HDMI 0x${m.ddc.hdmi.toString(16).toUpperCase().padStart(2, "0")}` : "";
  checks.push(
    m.ddc
      ? { state: "ok", label: "Perfil DDC", detail: codes }
      : { state: "idle", label: "Sin perfil DDC", detail: "se calibra, o se cambia a mano" },
  );
  const p = entry?.phase ?? { kind: "idle" as const };
  const home = entry?.home;
  switch (p.kind) {
    case "idle":
      checks.push({ state: "idle", label: "Ida y vuelta sin probar" });
      break;
    case "moving":
      checks.push({ state: "wait", label: `Pasándolo a la ${MACHINE_NAME[p.to]}…` });
      break;
    case "ask":
      checks.push({ state: "wait", label: `¿Ves la ${MACHINE_NAME[p.machine]}?` });
      break;
    case "returning":
      checks.push({ state: "wait", label: `Volviendo a la ${home ? MACHINE_NAME[home] : "otra"}…` });
      break;
    case "ask_home":
      checks.push({ state: "wait", label: `¿Ves la ${home ? MACHINE_NAME[home] : "otra"}?` });
      break;
    case "ok":
      checks.push({ state: "ok", label: "Ida y vuelta verificada", detail: "lo viste en las dos máquinas" });
      break;
    case "manual":
      checks.push({ state: "fail", label: "Modo manual asistido", detail: `${p.reason}: Tandem te pedirá cambiar la entrada con los botones del monitor` });
      break;
  }
  return checks;
}

// ---------------------------------------------------------------------------------------
// Cada paso

function connected(snap: Snapshot) {
  return snap.kind === "connected" ? snap : null;
}

function machineStep(snap: Snapshot): StepView {
  const v = empty("machine", "Tandem corre en las dos máquinas. Primero, esta.");
  const c = connected(snap);
  if (!c) {
    v.blocker = "no hay conexión con tandemd";
    v.checks.push({ state: "fail", label: "Agente de Tandem", detail: "sin conexión" });
    return v;
  }
  const here = c.caps.here;
  const os = here.os === "macos" ? "macOS" : here.os === "windows" ? "Windows" : here.os;
  v.checks.push({ state: "ok", label: `${MACHINE_NAME[c.agent]} · ${here.name}`, detail: `detectada por el sistema operativo (${os})` });
  const missing = here.input.missing;
  if (here.os === "macos") {
    if (missing.length === 0) {
      v.checks.push({ state: "ok", label: "Permisos de macOS", detail: "Accesibilidad y Monitorización de entrada concedidos" });
    }
    for (const p of missing) {
      const name = p === "accessibility" ? "Accesibilidad" : "Monitorización de entrada";
      v.checks.push({ state: "fail", label: `Permiso: ${name}`, detail: "falta; se comprueba solo en cuanto lo concedes" });
      v.buttons.push({ id: `perm:${p}`, label: `Abrir ajustes: ${name}`, enabled: true });
    }
  } else {
    v.checks.push({ state: "ok", label: "Permisos", detail: "esta máquina no necesita ninguno" });
  }
  v.verified = missing.length === 0;
  v.blocker = v.verified ? null : "faltan permisos de macOS";
  return v;
}

function pairStep(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("pair", "Las dos máquinas se emparejan una vez con un código que ves en las dos pantallas.");
  const c = connected(snap);
  if (!c) {
    v.blocker = "no hay conexión con tandemd";
    return v;
  }
  const { peer, pairing } = c;
  if (peer.kind === "paired") {
    v.checks.push({ state: "ok", label: `Emparejada con ${peer.name}` });
    if (peer.link) {
      v.checks.push({ state: "ok", label: `La ve por ${LINK_NAME[peer.link]}` });
      const ping = o.pair.ping;
      v.checks.push(
        ping
          ? { state: "ok", label: "Ping del enlace", detail: `${LINK_NAME[ping.link]} · ${ping.millis} ms` }
          : o.pair.pingError
            ? { state: "fail", label: "Ping del enlace", detail: o.pair.pingError }
            : { state: "idle", label: "Ping del enlace sin probar" },
      );
      v.buttons.push({ id: "ping", label: ping ? "Probar de nuevo" : "Probar el enlace", enabled: true });
      v.verified = ping !== null;
      v.blocker = ping ? null : "prueba el enlace";
    } else {
      v.checks.push({ state: "fail", label: "No la ve ahora", detail: "enciéndela y espera a que aparezca; Wi-Fi, USB4 o Ethernet" });
      v.blocker = "la otra máquina no responde";
    }
    return v;
  }
  switch (pairing.kind) {
    case "idle":
    case "failed":
      v.checks.push({ state: pairing.kind === "failed" ? "fail" : "idle", label: "Sin emparejar", detail: pairing.kind === "failed" ? pairing.reason : "abre Tandem también en la otra máquina" });
      v.buttons.push({ id: "pair:start", label: "Buscar la otra máquina", kind: "primary", enabled: true });
      break;
    case "searching":
      v.checks.push({ state: "wait", label: "Buscando a la otra máquina…", detail: "tiene que estar buscando también" });
      v.buttons.push({ id: "pair:cancel", label: "Cancelar", enabled: true });
      break;
    case "comparing":
      v.checks.push({ state: "wait", label: `Encontró a ${pairing.peer}`, detail: pairing.confirmed ? "esperando a que confirme la otra" : "¿es el mismo código en las dos pantallas?" });
      v.code = pairing.code;
      if (!pairing.confirmed) v.buttons.push({ id: "pair:confirm", label: "Es el mismo", kind: "primary", enabled: true });
      v.buttons.push({ id: "pair:cancel", label: "No coincide", kind: "danger", enabled: true });
      break;
    case "done":
      v.checks.push({ state: "ok", label: `Emparejada con ${pairing.peer}` });
      break;
  }
  if (!o.pair.skipped) v.buttons.push({ id: "pair:skip", label: "Seguir solo por ahora", enabled: pairing.kind !== "comparing" });
  v.verified = o.pair.skipped;
  if (o.pair.skipped) v.checks.push({ state: "idle", label: "Se sigue solo con esta máquina", detail: "sin la otra no se pueden vincular sus monitores ni probar el teclado" });
  v.blocker = v.verified ? null : "empareja las dos máquinas, o sigue solo por ahora";
  return v;
}

function keyboardStep(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("keyboard", "Presiona una tecla en el teclado que usas: la máquina que la reciba es la que lo tiene.");
  const c = connected(snap);
  const k = o.keyboard;
  if (!c) {
    v.blocker = "no hay conexión con tandemd";
    return v;
  }
  const peerKnown = c.peer.kind === "paired" && c.peer.link !== null;
  if (k.confirmed) {
    v.checks.push({ state: "ok", label: `El teclado y el mouse están en la ${MACHINE_NAME[k.confirmed]}`, detail: "lo confirmaste" });
    v.buttons.push({ id: "key:again", label: "Probar de nuevo", enabled: true });
    v.verified = true;
    return v;
  }
  if (k.heard) {
    v.checks.push({ state: "ok", label: `La tecla llegó a la ${MACHINE_NAME[k.heard]}`, detail: "¿es correcto?" });
    v.buttons.push({ id: `key:yes:${k.heard}`, label: `Sí, está en la ${MACHINE_NAME[k.heard]}`, kind: "primary", enabled: true });
    v.buttons.push({ id: `key:yes:${other(k.heard)}`, label: `No, está en la ${MACHINE_NAME[other(k.heard)]}`, enabled: true });
    v.buttons.push({ id: "key:again", label: "Probar de nuevo", enabled: true });
    v.blocker = "confirma dónde está el teclado";
    return v;
  }
  const listening = k.armed;
  v.checks.push({
    state: listening ? "wait" : "idle",
    label: listening ? "Escuchando en las dos máquinas… presiona una tecla" : "Sin probar",
    detail: peerKnown ? undefined : "la otra máquina no está enlazada: solo se escucha aquí; si el teclado está en ella, elígelo abajo",
  });
  if (!listening) v.buttons.push({ id: "key:start", label: "Empezar", kind: "primary", enabled: true });
  v.buttons.push({ id: "key:pick:pc", label: "Está en la PC", enabled: true });
  v.buttons.push({ id: "key:pick:mac", label: "Está en la Mac", enabled: true });
  v.checks.push({
    state: k.undefinedOk ? "ok" : "idle",
    label: k.undefinedOk ? "Sin definir" : "Si no responde, queda «sin definir»",
    detail: "nunca se supone que está en la Mac",
  });
  if (!k.undefinedOk) v.buttons.push({ id: "key:skip", label: "Dejar sin definir", enabled: true });
  v.verified = k.undefinedOk;
  v.blocker = v.verified ? null : "presiona una tecla, elige dónde está o déjalo sin definir";
  return v;
}

function monitorsStep(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("monitors", "Solo salen los monitores conectados ahora, juntando lo que ve cada máquina.");
  const c = connected(snap);
  if (!c) {
    v.blocker = "no hay conexión con tandemd";
    return v;
  }
  const m = o.monitors;
  const scan = m.scan;
  if (m.error) v.checks.push({ state: "fail", label: "No se pudo buscar", detail: m.error });
  else if (!scan) v.checks.push({ state: "wait", label: "Buscando monitores…" });
  else if (!scan.candidates.length) v.checks.push({ state: "idle", label: "Ningún monitor conectado", detail: "conéctalos y busca de nuevo" });
  if (scan && !scan.peer_known) {
    v.checks.push({ state: "idle", label: "Solo se ven los de esta máquina", detail: "la otra no está enlazada: no se pueden vincular las identidades" });
  }
  const agent = c.agent;
  for (const cand of scan?.candidates ?? []) {
    const ch = choiceOf(o, cand);
    const key = candidateKey(cand);
    const row: Row = { key, title: cand.label, subtitle: "", checks: [], questions: [], buttons: [] };
    row.subtitle = cand.pnp_ids.join(" + ") || "sin id PnP";
    if (cand.in_desk !== null) {
      row.checks.push({ state: "ok", label: "Ya está en el puesto" });
      v.rows.push(row);
      continue;
    }
    const both = cand.here !== null && cand.peer !== null;
    for (const [who, side] of [
      [agent, cand.here],
      [other(agent), cand.peer],
    ] as const) {
      row.checks.push({ state: side ? "ok" : "idle", label: `${MACHINE_NAME[who]}: ${identity(side)}`, detail: side?.inches ? `${side.inches}"` : undefined });
    }
    if (both) {
      const how = cand.basis === "Serial" ? "tienen el mismo número de serie" : "coinciden en modelo o tamaño";
      row.questions.push({
        text: `¿Este monitor de la ${MACHINE_NAME[agent]} es este de la ${MACHINE_NAME[other(agent)]}? (${how})`,
        options: [
          { id: `link:${key}:yes`, label: cand.basis === "Serial" ? "Sí, es el mismo (sugerido)" : "Sí, es el mismo", selected: ch.linked === true },
          { id: `link:${key}:no`, label: "No, son distintos", selected: ch.linked === false },
        ],
      });
    }
    if (ch.linked !== null) {
      for (const who of seenBy(cand, agent)) {
        const known = cand.wiring[who];
        const chosen = ch.wiring[who];
        row.questions.push({
          text: known
            ? `${MACHINE_NAME[who]}: lo ve por ${INPUT_NAME[known]} (detectado). Confirma el cable:`
            : `¿Por qué cable va a la ${MACHINE_NAME[who]}?`,
          options: (["displayport", "hdmi"] as Input[]).map((i) => ({
            id: `wire:${key}:${who}:${i}`,
            label: known === i ? `${INPUT_NAME[i]} (sugerido)` : INPUT_NAME[i],
            selected: chosen === i,
          })),
        });
      }
    }
    row.checks.push(
      cand.profile ? { state: "ok", label: "Perfil DDC en la base", detail: cand.profile } : { state: "idle", label: "Sin perfil DDC", detail: "se calibra en el paso 5" },
    );
    const ready = candidateReady(o, cand, agent);
    row.buttons.push({
      id: `add:${key}`,
      label: ch.add ? "Se agregará" : "No agregar",
      enabled: true,
      hint: ready ? undefined : (candidateBlocker(o, cand, agent) ?? undefined),
    });
    v.rows.push(row);
  }
  const pending = (scan?.candidates ?? []).filter((cand) => {
    const ch = choiceOf(o, cand);
    return ch.add && cand.in_desk === null;
  });
  const ready = pending.length > 0 && pending.every((cand) => candidateReady(o, cand, agent));
  const hasDesk = c.desk.monitors.length > 0;
  if (scan && scan.disconnected > 0) {
    v.checks.push({ state: "idle", label: `${scan.disconnected} monitor${scan.disconnected === 1 ? "" : "es"} del puesto no ${scan.disconnected === 1 ? "está" : "están"} conectado${scan.disconnected === 1 ? "" : "s"}` });
  }
  v.buttons.push({ id: "scan", label: "Buscar de nuevo", enabled: !m.scanning });
  if (pending.length) {
    v.buttons.push({
      id: "add",
      label: m.adding ? "Agregando…" : `Agregar ${pending.length} monitor${pending.length === 1 ? "" : "es"}`,
      kind: "primary",
      enabled: ready && !m.adding,
      hint: ready ? undefined : blockedReason(o, pending, agent),
    });
  }
  if (!hasDesk && !m.skipped) v.buttons.push({ id: "monitors:skip", label: "Seguir sin monitores", enabled: true });
  v.verified = hasDesk || m.skipped;
  v.blocker = v.verified ? null : pending.length ? "agrega los monitores elegidos" : "no hay monitores en el puesto";
  return v;
}

function verifyStepView(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("verify", "Cada monitor hace una ida y vuelta real: pasa a la otra máquina y regresa. Tú dices qué ves.");
  const c = connected(snap);
  if (!c) {
    v.blocker = "no hay conexión con tandemd";
    return v;
  }
  const busy = Object.values(o.verify).some((e) => ["moving", "ask", "returning", "ask_home"].includes(e.phase.kind));
  for (const m of c.desk.monitors) {
    const entry = o.verify[m.id];
    // Con el estado sin confirmar Tandem no sabe dónde está el monitor: lo dice el usuario.
    // Suponer la Mac mandaba el monitor de vuelta a donde no estaba (T-089).
    const known = homeOf(o, c.state, m.id);
    const home: Machine = known ?? "mac";
    const row: Row = { key: m.id, title: m.label, subtitle: [m.edid_model, ...(m.edid_aliases ?? [])].filter(Boolean).join(" + ") || m.id, checks: verifyChecks(m, entry), questions: [], buttons: [] };
    const p = entry?.phase;
    if (soloWired(m, entry)) {
      row.checks[row.checks.length - 1] = { state: "idle", label: "Solo cableado a una máquina: sin ida y vuelta", detail: "no hay otra entrada a la que pasarlo" };
    } else if (p?.kind === "ask") {
      row.questions.push({
        text: `¿Ves la ${MACHINE_NAME[p.machine]} en el ${m.label}?`,
        options: [
          { id: `ask:${m.id}:yes`, label: `Sí, veo la ${MACHINE_NAME[p.machine]}`, selected: false },
          { id: `ask:${m.id}:no`, label: "No, otra cosa", selected: false },
        ],
      });
    } else if (p?.kind === "ask_home") {
      row.questions.push({
        text: `¿Ves la ${MACHINE_NAME[entry!.home]} en el ${m.label}?`,
        options: [
          { id: `ask:${m.id}:yes`, label: `Sí, veo la ${MACHINE_NAME[entry!.home]}`, selected: false },
          { id: `ask:${m.id}:no`, label: "No, otra cosa", selected: false },
        ],
      });
    } else if (!verifyDone(entry) || p?.kind === "manual") {
      if (!m.ddc) {
        // La calibración va antes que cualquier cambio de entrada: la ida y vuelta, el estado
        // inicial y «Aplicar modo» usan los códigos de este monitor (T-089). Sin perfil no se
        // manda nada automático; o se calibra o se declara el modo manual asistido.
        row.questions.length = 0;
        if (c.agent === "pc") row.buttons.push({ id: `calibrate:${m.id}`, label: "Calibrar", kind: "primary", enabled: !busy });
        row.buttons.push({
          id: `manual:${m.id}`,
          label: "Cambiar a mano (modo manual asistido)",
          enabled: !busy,
          hint: c.agent === "pc" ? undefined : "la calibración se hace desde la PC",
        });
      } else if (known === null) {
        row.questions.push({
          text: `¿Qué ves ahora en el ${m.label}?`,
          options: (["pc", "mac"] as Machine[]).map((w) => ({ id: `home:${m.id}:${w}`, label: `La ${MACHINE_NAME[w]}`, selected: false })),
        });
      } else {
        const can = canRoundTrip(m, home);
        row.buttons.push({
          id: `verify:${m.id}`,
          label: p?.kind === "manual" ? "Probar de nuevo" : "Probar la ida y vuelta",
          kind: "primary",
          enabled: can && !busy && c.state.on[other(home)],
          hint: !can ? "solo está cableado a una máquina" : !c.state.on[other(home)] ? `la ${MACHINE_NAME[other(home)]} tiene que estar encendida` : undefined,
        });
      }
    }
    v.rows.push(row);
  }
  const rows = c.desk.monitors;
  v.verified = rows.length > 0 && rows.every((m) => soloWired(m, o.verify[m.id]) || verifyDone(o.verify[m.id]));
  if (!rows.length) {
    v.verified = true;
    v.checks.push({ state: "idle", label: "No hay monitores que probar" });
  }
  v.blocker = v.verified ? null : "prueba cada monitor";
  return v;
}

function initialStep(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("initial", "Elige cómo quieres empezar: Tandem manda la entrada de todos los monitores y no deja nada por suponer.");
  const c = connected(snap);
  if (!c) {
    v.blocker = "no hay conexión con tandemd";
    return v;
  }
  const i = o.initial;
  const confirmed = isConfirmed(c.desk, c.state);
  if (!c.desk.monitors.length) {
    v.verified = true;
    v.checks.push({ state: "idle", label: "No hay monitores: nada que aplicar" });
    return v;
  }
  for (const k of ["mac", "shared", "pc"] as ModeKey[]) {
    v.buttons.push({ id: `mode:${k}`, label: MODE_NAME[k], enabled: !i.applying, hint: i.mode === k ? "elegido" : undefined });
  }
  v.checks = [
    { state: i.mode ? "ok" : "idle", label: i.mode ? `Modo elegido: ${MODE_NAME[i.mode]}` : "Elige un modo" },
    i.applying
      ? { state: "wait", label: "Aplicando…", detail: "se manda la entrada de cada monitor" }
      : confirmed && i.applied
        ? { state: "ok", label: "Aplicado", detail: "Tandem sabe lo que muestra cada monitor" }
        : { state: "idle", label: "Sin aplicar", detail: "cada monitor está sin confirmar" },
  ];
  v.buttons.push({ id: "apply", label: "Aplicar modo", kind: "primary", enabled: i.mode !== null && !i.applying });
  if (i.applied && confirmed) {
    v.checks.push(
      i.looksRight === null
        ? { state: "wait", label: "¿Se ve como esperas?" }
        : i.looksRight
          ? { state: "ok", label: "Lo viste bien" }
          : { state: "fail", label: "No se ve como esperas", detail: "elige otro modo y aplícalo de nuevo" },
    );
    if (i.looksRight === null) {
      v.buttons.push({ id: "looks:yes", label: "Sí, se ve bien", enabled: true });
      v.buttons.push({ id: "looks:no", label: "No", enabled: true });
    }
  }
  v.verified = i.applied && confirmed && i.looksRight === true;
  v.blocker = v.verified ? null : "aplica un modo y confirma que se ve bien";
  return v;
}

/** Todo lo verificado, para el resumen (paso 7). */
export function summaryChecks(o: Onboarding, snap: Snapshot): Check[] {
  const c = connected(snap);
  if (!c) return [{ state: "fail", label: "Sin conexión con tandemd" }];
  const out: Check[] = [];
  out.push({ state: machineStep(snap).verified ? "ok" : "fail", label: `${MACHINE_NAME[c.agent]} · ${c.caps.here.name}`, detail: machineStep(snap).verified ? "permisos verificados" : "faltan permisos" });
  if (c.peer.kind === "paired") {
    const ping = o.pair.ping;
    out.push({ state: c.peer.link && ping ? "ok" : "fail", label: `${c.peer.name} emparejada`, detail: ping ? `${LINK_NAME[ping.link]} · ${ping.millis} ms` : "enlace sin probar" });
  } else out.push({ state: "idle", label: "Sin la otra máquina", detail: "se sigue solo con esta" });
  out.push(
    o.keyboard.confirmed
      ? { state: "ok", label: `Teclado y mouse en la ${MACHINE_NAME[o.keyboard.confirmed]}`, detail: "confirmado por ti" }
      : { state: "idle", label: "Teclado y mouse sin definir" },
  );
  for (const m of c.desk.monitors) {
    const e = o.verify[m.id];
    const ids = [m.edid_model, ...(m.edid_aliases ?? [])].filter(Boolean).join(" + ");
    const wiring = (["pc", "mac"] as Machine[]).flatMap((w) => (m.wiring[w] ? [`${PORT[m.wiring[w]!]} → ${MACHINE_NAME[w]}`] : [])).join(" · ");
    const mode = e?.phase.kind === "manual" ? "modo manual asistido" : e?.phase.kind === "ok" ? "ida y vuelta verificada" : soloWired(m, e) ? "solo cableado a una máquina: sin ida y vuelta" : "sin probar";
    out.push({ state: e?.phase.kind === "ok" ? "ok" : e?.phase.kind === "manual" ? "fail" : "idle", label: `${m.label} (${ids})`, detail: `${wiring} · ${m.ddc ? "perfil DDC" : "sin perfil"} · ${mode}` });
  }
  out.push(
    isConfirmed(c.desk, c.state) && c.desk.monitors.length
      ? { state: "ok", label: "Estado confirmado", detail: o.initial.mode ? `modo ${MODE_NAME[o.initial.mode]}` : undefined }
      : { state: c.desk.monitors.length ? "fail" : "idle", label: "Estado sin confirmar" },
  );
  return out;
}

function summaryStep(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("summary", "Esto es lo que quedó verificado.");
  v.checks = summaryChecks(o, snap);
  v.buttons.push({ id: "finish", label: "Terminar", kind: "primary", enabled: true });
  v.verified = true;
  return v;
}

/** Lo que se muestra en el paso actual. */
export function viewOf(o: Onboarding, snap: Snapshot): StepView {
  switch (o.step) {
    case "machine":
      return machineStep(snap);
    case "pair":
      return pairStep(o, snap);
    case "keyboard":
      return keyboardStep(o, snap);
    case "monitors":
      return monitorsStep(o, snap);
    case "verify":
      return verifyStepView(o, snap);
    case "initial":
      return initialStep(o, snap);
    case "summary":
      return summaryStep(o, snap);
  }
}

// ---------------------------------------------------------------------------------------
// Navegar

export const indexOf = (id: StepId) => STEPS.findIndex((s) => s.id === id);

/** El paso siguiente. Sin monitores se salta lo que depende de ellos. */
export function nextStep(o: Onboarding, snap: Snapshot): StepId {
  const i = indexOf(o.step);
  const hasDesk = snap.kind === "connected" && snap.desk.monitors.length > 0;
  const following = STEPS[Math.min(i + 1, STEPS.length - 1)]!.id;
  if (!hasDesk && (following === "verify" || following === "initial")) return "summary";
  return following;
}

export function previousStep(o: Onboarding, snap: Snapshot): StepId {
  const i = indexOf(o.step);
  const hasDesk = snap.kind === "connected" && snap.desk.monitors.length > 0;
  const before = STEPS[Math.max(i - 1, 0)]!.id;
  if (!hasDesk && (before === "initial" || before === "verify")) return "monitors";
  return before;
}

export function canAdvance(o: Onboarding, snap: Snapshot): boolean {
  return o.step !== "summary" && viewOf(o, snap).verified;
}

export function advanceTo(o: Onboarding, snap: Snapshot): Onboarding {
  return canAdvance(o, snap) ? { ...o, step: nextStep(o, snap), error: null } : o;
}

export function goBack(o: Onboarding, snap: Snapshot): Onboarding {
  return o.step === "machine" ? o : { ...o, step: previousStep(o, snap), error: null };
}

/** El pedido al agente de un monitor que el asistente quiere agregar. */
export function addAction(o: Onboarding, agent: Machine): Action {
  return { kind: "add_monitors", monitors: newMonitors(o, agent) };
}

/** Las respuestas de los botones de las filas de monitores. */
export function answerChoice(o: Onboarding, id: string, scan: MonitorScan | null = o.monitors.scan): Onboarding | null {
  const parts = id.split(":");
  const [kind] = parts;
  const patch = (key: string, change: Partial<MonitorChoice>): Onboarding => ({
    ...o,
    monitors: { ...o.monitors, choices: { ...o.monitors.choices, [key]: { ...o.monitors.choices[key], ...change } } },
  });
  if (kind === "link") {
    const [, key, yes] = parts;
    return patch(key!, { linked: yes === "yes" });
  }
  if (kind === "wire") {
    const [, key, who, input] = parts;
    const current = { ...(o.monitors.choices[key!]?.wiring ?? {}) };
    current[who as Machine] = input as Input;
    return patch(key!, { wiring: current });
  }
  if (kind === "add") {
    const [, key] = parts;
    const cand = scan?.candidates.find((c) => candidateKey(c) === key);
    const now = cand ? choiceOf(o, cand).add : true;
    return patch(key!, { add: !now });
  }
  return null;
}

/**
 * ¿Esta pulsación cuenta como «presiona una tecla»? Enter, espacio y tab sobre un botón son
 * del propio diálogo (activar o moverse), no del teclado que se está probando; las teclas
 * que solo modifican tampoco dicen nada.
 */
export function countsAsKeyProbe(e: { key: string; repeat?: boolean; targetIsButton: boolean }): boolean {
  if (e.repeat) return false;
  if (e.targetIsButton && ["Enter", " ", "Tab"].includes(e.key)) return false;
  return !["Shift", "Control", "Alt", "Meta", "CapsLock", "Dead", "Unidentified"].includes(e.key);
}

/** Cuánto se espera una respuesta de «¿ves la X?» antes de contarla como «no» y volver. */
export const ASK_TIMEOUT_MS = 20_000;

/** Reloj inyectable, para probar el plazo con tiempo simulado. */
export interface Clock {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

/**
 * Un plazo por monitor mientras espera una respuesta: si vence, `onExpire(monitor)`. Se arma
 * al entrar en `ask`/`ask_home` y se desarma al salir; `expireAll` es el cierre del diálogo
 * a mitad de la prueba. Toda prueba que cambia una entrada vuelve sola (CLAUDE.md).
 */
export class AskDeadline {
  private timers = new Map<string, unknown>();
  constructor(
    private clock: Clock,
    private onExpire: (monitor: string) => void,
    private ms = ASK_TIMEOUT_MS,
  ) {}

  /** Llamar con la fase de cada monitor después de cada cambio. */
  track(monitor: string, phase: VerifyPhase["kind"]) {
    const waiting = phase === "ask" || phase === "ask_home";
    const has = this.timers.has(monitor);
    if (waiting && !has) {
      this.timers.set(
        monitor,
        this.clock.set(() => {
          this.timers.delete(monitor);
          this.onExpire(monitor);
        }, this.ms),
      );
    } else if (!waiting && has) {
      this.clock.clear(this.timers.get(monitor));
      this.timers.delete(monitor);
    }
  }

  /** Vence ya todo lo que esperaba respuesta. */
  expireAll() {
    for (const [monitor, h] of [...this.timers]) {
      this.clock.clear(h);
      this.timers.delete(monitor);
      this.onExpire(monitor);
    }
  }

  get pending(): number {
    return this.timers.size;
  }
}

// ---- Cerrar el asistente a mitad del movimiento

/** Monitores que el asistente estaba pasando a la otra máquina al cerrarse, con su `home`. */
export type Abandoned = ReadonlyMap<string, Machine>;

/** Al cerrar: lo que todavía se mueve vuelve a `home` cuando el movimiento termine. */
export function abandonOnClose(verify: Record<string, VerifyEntry>, prev: Abandoned): Abandoned {
  const next = new Map(prev);
  for (const [monitor, e] of Object.entries(verify)) {
    if (e.phase.kind === "moving") next.set(monitor, e.home);
  }
  return next;
}

/**
 * El movimiento de `monitor` terminó. Con la marca y `committed`, hay que mandarlo de vuelta
 * a su `home` directamente (sin pasar por `verify`, que pudo reiniciarse); en cualquier caso
 * la marca se borra: no puede quedar para la siguiente prueba.
 */
export function moveEnded(ab: Abandoned, monitor: string, end: "committed" | "rolled_back" | "inconsistent" | "rejected"): { abandoned: Abandoned; returnTo: Machine | null } {
  const next = new Map(ab);
  const home = next.get(monitor) ?? null;
  next.delete(monitor);
  return { abandoned: next, returnTo: end === "committed" ? home : null };
}

/** Al reabrir: solo se conservan las marcas de movimientos que siguen en vuelo. */
export function keepInFlight(ab: Abandoned, inFlight: string | null): Abandoned {
  return new Map([...ab].filter(([m]) => m === inFlight));
}
