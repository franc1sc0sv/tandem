// El asistente de configuración inicial (DESIGN §5, sin monitores): un paso por pantalla y
// cada paso verificado antes de pasar al siguiente: (1) this machine y sus permisos, (2)
// emparejar y ver el ping del enlace, (3) «presiona una tecla» y (4) el resumen. El acomodo
// de pantallas llega con T-117. Aquí, lo que no depende del DOM: el estado del asistente, lo
// que se muestra en cada paso (`viewOf`) y las reglas de cuándo se puede avanzar. El
// asistente de 6 pasos de v0.3 (T-119) lo usa desde Svelte.

import type { Link, Machine, Snapshot } from "./model";
import { MISSING_HELP, grantTo, permissionLabel } from "./permissions";
import { LINK_NAME, machineName, other } from "./machines";

export type StepId = "machine" | "pair" | "keyboard" | "summary";

export const STEPS: { id: StepId; title: string }[] = [
  { id: "machine", title: "This machine" },
  { id: "pair", title: "The other machine" },
  { id: "keyboard", title: "Keyboard and mouse" },
  { id: "summary", title: "Summary" },
];

// ---------------------------------------------------------------------------------------
// Estado

export interface PairFacts {
  /** El último ping al otro agente, con el enlace por el que fue. */
  ping: { link: Link; millis: number } | null;
  pingError: string | null;
  /** «Continue alone for now». */
  skipped: boolean;
}

export interface KeyFacts {
  /** Las dos máquinas están escuchando. */
  armed: boolean;
  /** La máquina que recibió la tecla. */
  heard: Machine | null;
  /** Lo que el usuario confirmó (o eligió a mano). */
  confirmed: Machine | null;
  /** «Leave undefined»: a machine is never assumed. */
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
  const v = empty("machine", "Tandem runs on both machines. First, this one.");
  const c = connected(snap);
  if (!c) {
    v.blocker = "there is no connection to tandemd";
    v.checks.push({ state: "fail", label: "Tandem agent", detail: "offline" });
    return v;
  }
  const here = c.caps.here;
  const os = here.os === "macos" ? "macOS" : "Windows";
  v.checks.push({ state: "ok", label: here.name, detail: `detected by the operating system (${os})` });
  const missing = here.input.missing;
  if (here.os === "macos") {
    if (missing.length === 0) {
      v.checks.push({ state: "ok", label: "macOS permissions", detail: "Accessibility and Input Monitoring granted" });
    }
    for (const p of missing) {
      v.checks.push({ state: "fail", label: `Permission: ${permissionLabel(p)}`, detail: "missing; it is checked automatically as soon as you grant it" });
    }
    if (missing.length > 0) {
      // macOS concede los permisos a un binario concreto: se le dice cuál es, sin terminal (T-090).
      const binary = grantTo(here);
      if (binary) v.checks.push({ state: "wait", label: "Agent binary", detail: binary });
      v.checks.push({ state: "wait", label: "How to grant", detail: MISSING_HELP });
      v.buttons.push({ id: "perm:request", label: "Request permissions", kind: "primary", enabled: true, hint: "macOS shows its dialog for tandemd" });
      for (const p of missing) v.buttons.push({ id: `perm:${p}`, label: `Open settings: ${permissionLabel(p)}`, enabled: true });
      v.buttons.push({ id: "perm:restart", label: "Restart agent", enabled: true, hint: "so the permissions you just gave take effect" });
    }
  } else {
    v.checks.push({ state: "ok", label: "Permissions", detail: "this machine needs none" });
  }
  v.verified = missing.length === 0;
  v.blocker = v.verified ? null : "macOS permissions are missing";
  return v;
}

function pairStep(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("pair", "The two machines pair once with a code you see on both screens.");
  const c = connected(snap);
  if (!c) {
    v.blocker = "there is no connection to tandemd";
    return v;
  }
  const { peer, pairing } = c;
  if (peer.kind === "paired") {
    v.checks.push({ state: "ok", label: `Paired with ${peer.name}` });
    if (peer.link) {
      v.checks.push({ state: "ok", label: `It sees it over ${LINK_NAME[peer.link]}` });
      const ping = o.pair.ping;
      v.checks.push(
        ping
          ? { state: "ok", label: "Link ping", detail: `${LINK_NAME[ping.link]} · ${ping.millis} ms` }
          : o.pair.pingError
            ? { state: "fail", label: "Link ping", detail: o.pair.pingError }
            : { state: "idle", label: "Link ping not tested" },
      );
      v.buttons.push({ id: "ping", label: ping ? "Test again" : "Test the link", enabled: true });
      v.verified = ping !== null;
      v.blocker = ping ? null : "test the link";
    } else {
      v.checks.push({ state: "fail", label: "It does not see it right now", detail: "power it on and wait for it to appear; USB-C, Ethernet or Wi-Fi" });
      v.blocker = "the other machine does not respond";
    }
    return v;
  }
  switch (pairing.kind) {
    case "idle":
    case "failed":
      v.checks.push({ state: pairing.kind === "failed" ? "fail" : "idle", label: "Not paired", detail: pairing.kind === "failed" ? pairing.reason : "open Tandem on the other machine too" });
      v.buttons.push({ id: "pair:start", label: "Look for the other machine", kind: "primary", enabled: true });
      break;
    case "searching":
      v.checks.push({ state: "wait", label: "Looking for the other machine…", detail: "it has to be looking too" });
      v.buttons.push({ id: "pair:cancel", label: "Cancel", enabled: true });
      break;
    case "comparing":
      v.checks.push({ state: "wait", label: `Found ${pairing.peer}`, detail: pairing.confirmed ? "waiting for the other one to confirm" : "is it the same code on both screens?" });
      v.code = pairing.code;
      if (!pairing.confirmed) v.buttons.push({ id: "pair:confirm", label: "It is the same", kind: "primary", enabled: true });
      v.buttons.push({ id: "pair:cancel", label: "It does not match", kind: "danger", enabled: true });
      break;
    case "done":
      v.checks.push({ state: "ok", label: `Paired with ${pairing.peer}` });
      break;
  }
  if (!o.pair.skipped) v.buttons.push({ id: "pair:skip", label: "Continue alone for now", enabled: pairing.kind !== "comparing" });
  v.verified = o.pair.skipped;
  if (o.pair.skipped) v.checks.push({ state: "idle", label: "Continuing with this machine alone", detail: "without the other one the keyboard cannot be tested between the two" });
  v.blocker = v.verified ? null : "pair the two machines, or continue alone for now";
  return v;
}

function keyboardStep(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("keyboard", "Press a key on the keyboard you use: the machine that receives it is the one that has it.");
  const c = connected(snap);
  const k = o.keyboard;
  if (!c) {
    v.blocker = "there is no connection to tandemd";
    return v;
  }
  const peerKnown = c.peer.kind === "paired" && c.peer.link !== null;
  if (k.confirmed) {
    v.checks.push({ state: "ok", label: `The keyboard and mouse are on ${nameOf(c, k.confirmed)}`, detail: "you confirmed it" });
    v.buttons.push({ id: "key:again", label: "Test again", enabled: true });
    v.verified = true;
    return v;
  }
  if (k.heard) {
    v.checks.push({ state: "ok", label: `The key arrived on ${nameOf(c, k.heard)}`, detail: "is that right?" });
    v.buttons.push({ id: `key:yes:${k.heard}`, label: `Yes, it is on ${nameOf(c, k.heard)}`, kind: "primary", enabled: true });
    v.buttons.push({ id: `key:yes:${other(k.heard)}`, label: `No, it is on ${nameOf(c, other(k.heard))}`, enabled: true });
    v.buttons.push({ id: "key:again", label: "Test again", enabled: true });
    v.blocker = "confirm where the keyboard is";
    return v;
  }
  const listening = k.armed;
  v.checks.push({
    state: listening ? "wait" : "idle",
    label: listening ? "Listening on both machines… press a key" : "Not tested",
    detail: peerKnown ? undefined : "the other machine is not linked: it only listens here; if the keyboard is on it, choose it below",
  });
  if (!listening) v.buttons.push({ id: "key:start", label: "Start", kind: "primary", enabled: true });
  for (const m of [c.agent, other(c.agent)]) v.buttons.push({ id: `key:pick:${m}`, label: `It is on ${nameOf(c, m)}`, enabled: true });
  v.checks.push({
    state: k.undefinedOk ? "ok" : "idle",
    label: k.undefinedOk ? "Undefined" : "If it does not respond, it stays “undefined”",
    detail: "a machine is never assumed",
  });
  if (!k.undefinedOk) v.buttons.push({ id: "key:skip", label: "Leave undefined", enabled: true });
  v.verified = k.undefinedOk;
  v.blocker = v.verified ? null : "press a key, choose where it is or leave it undefined";
  return v;
}

export function summaryChecks(o: Onboarding, snap: Snapshot): Check[] {
  const c = connected(snap);
  if (!c) return [{ state: "fail", label: "Not connected to tandemd" }];
  const out: Check[] = [];
  const machine = machineStep(snap).verified;
  out.push({ state: machine ? "ok" : "fail", label: c.caps.here.name, detail: machine ? "permissions verified" : "permissions missing" });
  if (c.peer.kind === "paired") {
    const ping = o.pair.ping;
    out.push({ state: c.peer.link && ping ? "ok" : "fail", label: `${c.peer.name} paired`, detail: ping ? `${LINK_NAME[ping.link]} · ${ping.millis} ms` : "link not tested" });
  } else out.push({ state: "idle", label: "Without the other machine", detail: "continuing with this one alone" });
  out.push(
    o.keyboard.confirmed
      ? { state: "ok", label: `Keyboard and mouse on ${nameOf(c, o.keyboard.confirmed)}`, detail: "confirmed by you" }
      : { state: "idle", label: "Keyboard and mouse undefined" },
  );
  return out;
}

function summaryStep(o: Onboarding, snap: Snapshot): StepView {
  const v = empty("summary", "This is what was verified.");
  v.checks = summaryChecks(o, snap);
  v.buttons.push({ id: "finish", label: "Finish", kind: "primary", enabled: true });
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
