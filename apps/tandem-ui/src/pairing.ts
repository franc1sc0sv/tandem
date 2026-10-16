// La pantalla de emparejamiento a partir del par y del avance que manda el agente. Pura,
// para probarla sin DOM.

import type { Machine, PairingView, PeerView } from "./model";
import { MACHINE_NAME, other } from "./scene";

export type PairScreen =
  /** Sin par y sin emparejar: qué va a pasar y el botón para empezar. */
  | { kind: "intro"; other: string }
  /** Ya emparejado: con quién, y desemparejar. */
  | { kind: "paired"; name: string }
  /** Pidió desemparejar: se confirma antes. */
  | { kind: "unpair"; name: string }
  | { kind: "searching"; other: string; hint: string }
  | { kind: "compare"; digits: [string, string]; peer: string; confirmed: boolean }
  | { kind: "done"; peer: string }
  | { kind: "failed"; reason: string };

/** El estado de la línea entre las dos máquinas que dibuja el diálogo. */
export type PairLine = "idle" | "searching" | "found" | "linked" | "broken";

/** Qué muestra el diálogo. `agent` es la máquina de esta ventana. */
export function pairScreen(agent: Machine, peer: PeerView, pairing: PairingView, askUnpair = false): PairScreen {
  const theOther = MACHINE_NAME[other(agent)];
  switch (pairing.kind) {
    case "searching":
      return {
        kind: "searching",
        other: theOther,
        // La PC espera y la Mac la busca: lo que tiene que hacer el usuario es lo mismo.
        hint: `En la ${theOther}, abre Tandem y pulsa «Emparejar».`,
      };
    case "comparing":
      return { kind: "compare", digits: splitCode(pairing.code), peer: pairing.peer, confirmed: pairing.confirmed };
    case "done":
      return { kind: "done", peer: pairing.peer };
    case "failed":
      return { kind: "failed", reason: pairing.reason };
    case "idle":
      if (peer.kind === "paired") return askUnpair ? { kind: "unpair", name: peer.name } : { kind: "paired", name: peer.name };
      return { kind: "intro", other: theOther };
  }
}

/** 6 dígitos en dos grupos de 3, que se comparan mejor de un vistazo. */
export function splitCode(code: string): [string, string] {
  const digits = code.replace(/\D/g, "").padStart(6, "0").slice(-6);
  return [digits.slice(0, 3), digits.slice(3)];
}

export function pairLine(screen: PairScreen): PairLine {
  switch (screen.kind) {
    case "searching":
      return "searching";
    case "compare":
      return "found";
    case "done":
    case "paired":
      return "linked";
    case "failed":
    case "unpair":
      return "broken";
    case "intro":
      return "idle";
  }
}

/** Hay un emparejamiento vivo: cerrar el diálogo lo cancela (y avisa a la otra máquina). */
export function isActive(pairing: PairingView): boolean {
  return pairing.kind === "searching" || pairing.kind === "comparing";
}

/** Cerrar un resultado ya mostrado lo deja en reposo en el agente. */
export function needsDismiss(pairing: PairingView): boolean {
  return pairing.kind === "done" || pairing.kind === "failed";
}

/**
 * Abrir el diálogo solo: cuando el emparejamiento pasa a estar vivo (por ejemplo, la
 * ventana se abrió con uno en curso) o aparece el código. Nunca para un resultado viejo.
 */
export function shouldOpen(before: PairingView | null, after: PairingView): boolean {
  if (!isActive(after)) return false;
  if (before === null || !isActive(before)) return true;
  return before.kind === "searching" && after.kind === "comparing";
}

/** El texto del botón de la píldora del enlace. */
export function pairButton(peer: PeerView): string {
  return peer.kind === "paired" ? "Par" : "Emparejar";
}
