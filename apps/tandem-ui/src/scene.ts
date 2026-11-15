// Lo que la escena muestra a partir del estado del agente. Puro, para probarlo sin DOM.

import type { Caps, Desk, DeskState, InputView, Link, Machine, PeerView, Step, TxnView } from "./model";

export const MACHINES: Machine[] = ["mac", "pc"];
export const MACHINE_NAME: Record<Machine, string> = { mac: "Mac", pc: "PC" };

export const LINK_NAME: Record<Link, string> = { usb4: "USB4", ethernet: "Ethernet", wifi: "Wi-Fi" };

/**
 * Los monitores y los modos están en pausa hasta el firmware propio del 34" (DESIGN §8):
 * se dibujan, deshabilitados, con esta leyenda.
 */
export const SOON = "Próximamente: firmware propio";

/** Los dos monitores del puesto, solo para dibujarlos deshabilitados. */
export const MONITORS_SOON: { label: string; wide: boolean }[] = [
  { label: '34"', wide: true },
  { label: '32"', wide: false },
];

/** Las tres tarjetas de modo, deshabilitadas. */
export const MODES_SOON = ["Solo Mac", "Compartido", "Solo Windows"] as const;

export function other(m: Machine): Machine {
  return m === "mac" ? "pc" : "mac";
}

/** La píldora del enlace: con qué se ve al otro agente, o por qué no. */
export function peerPill(peer: PeerView): { text: string; title: string; ok: boolean } {
  if (peer.kind === "unpaired") {
    return { text: "sin par", title: "Sin par emparejado: esta máquina trabaja sola", ok: false };
  }
  if (peer.link === null) {
    return { text: "sin enlace", title: `Ningún enlace ve a ${peer.name}`, ok: false };
  }
  const note = peer.link === "wifi" ? " (solo control: los servicios requieren cable)" : "";
  return { text: LINK_NAME[peer.link], title: `${peer.name} por ${LINK_NAME[peer.link]}${note}`, ok: true };
}

export const PERMISSION_NAME: Record<InputView["missing"][number], string> = {
  accessibility: "Accesibilidad",
  input_monitoring: "Monitorización de entrada",
};

/** El chip de teclado y mouse de una máquina; `null` si no se muestra. */
export interface RoleChip {
  /** `keyboard`: el teclado está en esta máquina; `remote`: lo recibe de la otra. */
  kind: "keyboard" | "remote" | "warn" | "idle";
  icon: "keyboard" | "cursor" | "warn";
  text: string;
  title: string;
}

/**
 * El chip de `who` visto desde el agente de `agent`. Con la otra apagada solo lleva chip la
 * del agente, si tiene el teclado. Los permisos que faltan y los problemas son de esta
 * máquina: su chip los avisa antes que nada. No hay «servidor» ni «cliente» (T-084): el
 * teclado está en una máquina y la otra lo recibe, en cualquiera de las dos direcciones.
 */
export function roleChip(input: InputView, agent: Machine, who: Machine, peerOn: boolean): RoleChip | null {
  const mine = who === agent;
  if (mine && input.missing.length > 0) {
    const names = input.missing.map((p) => PERMISSION_NAME[p]).join(" y ");
    return {
      kind: "warn",
      icon: "warn",
      text: "faltan permisos",
      title: `Teclado y mouse: falta ${names} (Ajustes del Sistema → Privacidad y seguridad / System Settings → Privacy & Security). Se piden y se reinicia el agente desde el paso «Esta máquina» del asistente o desde Permisos`,
    };
  }
  if (mine && input.problem) {
    return { kind: "warn", icon: "warn", text: "sin teclado y mouse", title: `Teclado y mouse: ${input.problem}` };
  }
  // Sin hub, configuración ni respuesta del usuario no se sabe dónde está: nunca se supone
  // la Mac (T-087). Lo dice el chip de esta máquina; el de la otra no.
  if (!input.defined) {
    return mine
      ? {
          kind: "idle",
          icon: "keyboard",
          text: "Teclado sin definir",
          title: "No se sabe en qué máquina está el teclado: se dice en el asistente (presionando una tecla)",
        }
      : null;
  }
  const has = input.server === who;
  if (!peerOn) {
    return mine && has
      ? { kind: "keyboard", icon: "keyboard", text: "Teclado aquí", title: `El teclado y el mouse están en la ${MACHINE_NAME[who]}` }
      : null;
  }
  const base = has
    ? { kind: "keyboard" as const, icon: "keyboard" as const, text: `${mine ? "Teclado aquí" : "Tiene el teclado"}${input.locked ? " · bloqueado" : ""}` }
    : { kind: "remote" as const, icon: "cursor" as const, text: `Teclado de la ${MACHINE_NAME[other(who)]}` };
  const titles = [has ? `La ${MACHINE_NAME[who]} tiene el teclado y el mouse` : `La ${MACHINE_NAME[who]} usa el teclado y el mouse de la ${MACHINE_NAME[other(who)]}`];
  if (has && input.locked) titles.push("el cursor está bloqueado en su pantalla (atajo)");
  if (!input.session) titles.push("sin sesión con el otro agente");
  return { ...base, kind: input.session ? base.kind : "idle", title: titles.join(" · ") };
}

// ---------------------------------------------------------------------------------------
// Sin centro fijo (T-084): «esta máquina» y «la otra», con lo que cada una anuncia.

/**
 * Las dos tarjetas de la escena, en el orden del acomodo (DESIGN §7): la Mac a la izquierda
 * salvo que el puesto diga otra cosa.
 */
export function sides(agent: Machine, desk?: Desk): { left: Machine; right: Machine; here: Machine; peer: Machine } {
  const macLeft = (desk?.arrangement.mac_side ?? "left") === "left";
  return { left: macLeft ? "mac" : "pc", right: macLeft ? "pc" : "mac", here: agent, peer: other(agent) };
}

/** Las pantallas declaradas de `who` en el puesto (vacío: valen las que lee ella misma). */
export function declaredScreens(desk: Desk, who: Machine): Desk["screens"] {
  return desk.screens.filter((s) => s.machine === who);
}

/** Lo que anunció `who`, si se sabe. */
export function capsOf(caps: Caps, who: Machine): Caps["here"] | null {
  if (caps.here.machine === who) return caps.here;
  return caps.peer && caps.peer.machine === who ? caps.peer : null;
}

export type PeerPower = "on" | "off" | "boot";

/** La otra máquina: arrancando mientras corre su encendido. */
export function peerPower(state: DeskState, agent: Machine, p: Progress | null): PeerPower {
  const peer = other(agent);
  if (state.on[peer]) return "on";
  const waking = p && isRunning(p) && p.steps.some((s, i) => s.step === "power_on" && s.machine === peer && p.status[i] === "run");
  return waking ? "boot" : "off";
}

/** El estado escrito bajo el nombre de cada tarjeta. La del agente nunca está apagada. */
export function machineState(state: DeskState, agent: Machine, who: Machine, p: Progress | null): string {
  if (who === agent) return "esta máquina";
  return { on: "encendida", boot: "arrancando…", off: "apagada" }[peerPower(state, agent, p)];
}

export interface PowerButton {
  /** Hay algo que se pueda pedir (aunque ahora esté ocupado). */
  visible: boolean;
  /** Lo que se pide al pulsarlo; `null` mientras arranca. */
  on: boolean | null;
  label: string;
  power: PeerPower;
}

/**
 * El botón de encendido de la otra máquina: encenderla si esta tiene Wake-on-LAN y la
 * otra se deja despertar, apagarla si ella sabe apagarse limpio. Sin su anuncio se supone
 * que se la puede despertar (si no, el paso falla con el motivo).
 */
export function powerButton(caps: Caps, state: DeskState, agent: Machine, p: Progress | null): PowerButton {
  const peer = other(agent);
  const name = MACHINE_NAME[peer];
  const power = peerPower(state, agent, p);
  const theirs = capsOf(caps, peer);
  if (power === "boot") return { visible: true, on: null, label: `La ${name} está arrancando`, power };
  if (power === "on") {
    const can = theirs?.power.power_off ?? false;
    return { visible: can, on: false, label: `Apagar la ${name}`, power };
  }
  // Hace falta que esta mande el Wake-on-LAN y que la otra se deje despertar.
  const can = (capsOf(caps, agent)?.power.wake_peer ?? true) && (theirs?.power.wakeable ?? true);
  return { visible: can, on: true, label: `Encender la ${name}`, power };
}

// ---------------------------------------------------------------------------------------
// Avance de una transacción (encender o apagar).

export type StepStatus = "pending" | "run" | "done" | "failed";

export type Outcome = { kind: "running" } | { kind: "committed" } | { kind: "failed"; error: string };

export interface Progress {
  txn: number;
  steps: Step[];
  status: StepStatus[];
  outcome: Outcome;
  /** El error del paso que falló, hasta que llegue el desenlace. */
  error?: string;
}

/** Texto corto de un paso. */
export function stepLabel(s: Step): string {
  return `${s.step === "power_on" ? "Encender" : "Apagar"} la ${MACHINE_NAME[s.machine]}`;
}

/** Avanza el progreso con un aviso. Los avisos de otra transacción se ignoran. */
export function advance(p: Progress | null, e: TxnView): Progress | null {
  if (e.type === "started") {
    return { txn: e.txn, steps: e.steps, status: e.steps.map(() => "pending"), outcome: { kind: "running" } };
  }
  if (!p || p.txn !== e.txn) return p;
  const status = [...p.status];
  switch (e.type) {
    case "step_started":
      status[e.index] = "run";
      return { ...p, status };
    case "step_done":
      status[e.index] = "done";
      return { ...p, status };
    case "step_failed":
      status[e.index] = "failed";
      return { ...p, status, error: e.error };
    case "committed":
      return { ...p, outcome: { kind: "committed" } };
    case "failed":
      return { ...p, outcome: { kind: "failed", error: p.error ?? "un paso falló" } };
  }
}

export function isRunning(p: Progress | null): boolean {
  return p !== null && p.outcome.kind === "running";
}
