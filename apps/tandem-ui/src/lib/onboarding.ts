// El asistente de configuración inicial (DESIGN §5, sin monitores): un paso por pantalla y
// cada paso verificado antes de pasar al siguiente: (1) esta máquina y sus permisos, (2)
// emparejar y ver el ping del enlace, (3) «presiona una tecla» y (4) el resumen. El acomodo
// de pantallas llega con T-117. Aquí, lo que no depende del DOM: el estado del asistente, lo
// que se muestra en cada paso (`viewOf`) y las reglas de cuándo se puede avanzar. El
// asistente de 6 pasos de v0.3 (T-119) lo usa desde Svelte.

import type { Link, Machine, Snapshot } from "./model";
import { MISSING_HELP, grantTo, permissionLabel } from "./permissions";
import { LINK_NAME, machineName, other } from "./machines";

export type StepId = "machine" | "pair" | "keyboard" | "summary";

export const STEPS: { id: StepId; title: string }[] = [
  { id: "machine", title: "Esta máquina" },
  { id: "pair", title: "La otra máquina" },
  { id: "keyboard", title: "Teclado y mouse" },
  { id: "summary", title: "Resumen" },
];

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
  /** «Dejar sin definir»: nunca se supone una máquina. */
  undefinedOk: boolean;
}

export interface Onboarding {
  step: StepId;
  pair: PairFacts;
  keyboard: KeyFacts;
  /** Un mensaje del último pedido que el agente rechazó. */
  error: string | null;
}

export function begin(step: StepId = "machine"): Onboarding {
  return {
    step,
    pair: { ping: null, pingError: null, skipped: false },
    keyboard: { armed: false, heard: null, confirmed: null, undefinedOk: false },
    error: null,
  };
}

/** Si el asistente debe abrirse solo: una instalación limpia (sin par y sin teclado dicho). */
export function shouldOpen(snap: Snapshot): boolean {
  return snap.kind === "connected" && snap.peer.kind === "unpaired" && !snap.input.defined;
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

export interface StepView {
  id: StepId;
  title: string;
  lead: string;
  checks: Check[];
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
  code: null,
  buttons: [],
  verified: false,
  blocker: null,
});

// ---------------------------------------------------------------------------------------
// Cada paso

function connected(snap: Snapshot) {
  return snap.kind === "connected" ? snap : null;
}

type Connected = Extract<Snapshot, { kind: "connected" }>;

/** El nombre de cada lado sale de su anuncio (D2): nada de «la Mac» o «la PC». */
const nameOf = (c: Connected, m: Machine) => machineName(m, c.caps, c.desk);

function machineStep(snap: Snapshot): StepView {
  const v = empty("machine", "Tandem corre en las dos máquinas. Primero, esta.");
  const c = connected(snap);
  if (!c) {
    v.blocker = "no hay conexión con tandemd";
    v.checks.push({ state: "fail", label: "Agente de Tandem", detail: "sin conexión" });
    return v;
  }
  const here = c.caps.here;
  const os = here.os === "macos" ? "macOS" : "Windows";
  v.checks.push({ state: "ok", label: here.name, detail: `detectada por el sistema operativo (${os})` });
  const missing = here.input.missing;
  if (here.os === "macos") {
    if (missing.length === 0) {
      v.checks.push({ state: "ok", label: "Permisos de macOS", detail: "Accesibilidad y Monitorización de entrada concedidos" });
    }
    for (const p of missing) {
      v.checks.push({ state: "fail", label: `Permiso: ${permissionLabel(p)}`, detail: "falta; se comprueba solo en cuanto lo concedes" });
    }
    if (missing.length > 0) {
      // macOS concede los permisos a un binario concreto: se le dice cuál es, sin terminal (T-090).
      const binary = grantTo(here);
      if (binary) v.checks.push({ state: "wait", label: "Binario del agente", detail: binary });
      v.checks.push({ state: "wait", label: "Cómo conceder", detail: MISSING_HELP });
      v.buttons.push({ id: "perm:request", label: "Pedir permisos", kind: "primary", enabled: true, hint: "macOS muestra su diálogo para tandemd" });
      for (const p of missing) v.buttons.push({ id: `perm:${p}`, label: `Abrir ajustes: ${permissionLabel(p)}`, enabled: true });
      v.buttons.push({ id: "perm:restart", label: "Reiniciar agente", enabled: true, hint: "para que valgan los permisos recién dados" });
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
      v.checks.push({ state: "fail", label: "No la ve ahora", detail: "enciéndela y espera a que aparezca; USB-C, Ethernet o Wi-Fi" });
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
  if (o.pair.skipped) v.checks.push({ state: "idle", label: "Se sigue solo con esta máquina", detail: "sin la otra no se puede probar el teclado entre las dos" });
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
    v.checks.push({ state: "ok", label: `El teclado y el mouse están en ${nameOf(c, k.confirmed)}`, detail: "lo confirmaste" });
    v.buttons.push({ id: "key:again", label: "Probar de nuevo", enabled: true });
    v.verified = true;
    return v;
  }
  if (k.heard) {
    v.checks.push({ state: "ok", label: `La tecla llegó a ${nameOf(c, k.heard)}`, detail: "¿es correcto?" });
    v.buttons.push({ id: `key:yes:${k.heard}`, label: `Sí, está en ${nameOf(c, k.heard)}`, kind: "primary", enabled: true });
    v.buttons.push({ id: `key:yes:${other(k.heard)}`, label: `No, está en ${nameOf(c, other(k.heard))}`, enabled: true });
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
  for (const m of [c.agent, other(c.agent)]) v.buttons.push({ id: `key:pick:${m}`, label: `Está en ${nameOf(c, m)}`, enabled: true });
  v.checks.push({
    state: k.undefinedOk ? "ok" : "idle",
    label: k.undefinedOk ? "Sin definir" : "Si no responde, queda «sin definir»",
    detail: "nunca se supone una máquina",
  });
  if (!k.undefinedOk) v.buttons.push({ id: "key:skip", label: "Dejar sin definir", enabled: true });
  v.verified = k.undefinedOk;
  v.blocker = v.verified ? null : "presiona una tecla, elige dónde está o déjalo sin definir";
  return v;
}

export function summaryChecks(o: Onboarding, snap: Snapshot): Check[] {
  const c = connected(snap);
  if (!c) return [{ state: "fail", label: "Sin conexión con tandemd" }];
  const out: Check[] = [];
  const machine = machineStep(snap).verified;
  out.push({ state: machine ? "ok" : "fail", label: c.caps.here.name, detail: machine ? "permisos verificados" : "faltan permisos" });
  if (c.peer.kind === "paired") {
    const ping = o.pair.ping;
    out.push({ state: c.peer.link && ping ? "ok" : "fail", label: `${c.peer.name} emparejada`, detail: ping ? `${LINK_NAME[ping.link]} · ${ping.millis} ms` : "enlace sin probar" });
  } else out.push({ state: "idle", label: "Sin la otra máquina", detail: "se sigue solo con esta" });
  out.push(
    o.keyboard.confirmed
      ? { state: "ok", label: `Teclado y mouse en ${nameOf(c, o.keyboard.confirmed)}`, detail: "confirmado por ti" }
      : { state: "idle", label: "Teclado y mouse sin definir" },
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
    case "summary":
      return summaryStep(o, snap);
  }
}

// ---------------------------------------------------------------------------------------
// Navegar

export const indexOf = (id: StepId) => STEPS.findIndex((s) => s.id === id);

export function nextStep(o: Onboarding): StepId {
  return STEPS[Math.min(indexOf(o.step) + 1, STEPS.length - 1)]!.id;
}

export function previousStep(o: Onboarding): StepId {
  return STEPS[Math.max(indexOf(o.step) - 1, 0)]!.id;
}

export function canAdvance(o: Onboarding, snap: Snapshot): boolean {
  return o.step !== "summary" && viewOf(o, snap).verified;
}

export function advanceTo(o: Onboarding, snap: Snapshot): Onboarding {
  return canAdvance(o, snap) ? { ...o, step: nextStep(o), error: null } : o;
}

export function goBack(o: Onboarding): Onboarding {
  return o.step === "machine" ? o : { ...o, step: previousStep(o), error: null };
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
