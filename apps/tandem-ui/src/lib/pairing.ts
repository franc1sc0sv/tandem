// La pantalla de emparejamiento a partir del par y del avance que manda el agente. Pura,
// para probarla sin DOM.

import type { PairingView, PeerView } from "./model";

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

/**
 * Qué muestra el diálogo. `theOther` es el nombre de the other machine (de su anuncio, o
 * «the other machine» si todavía no se la conoce): nada se supone por sistema (D2).
 */
export function pairScreen(theOther: string, peer: PeerView, pairing: PairingView, askUnpair = false): PairScreen {
  switch (pairing.kind) {
    case "searching":
      return {
        kind: "searching",
        other: theOther,
        // Una espera y la otra la busca: lo que tiene que hacer el usuario es lo mismo.
        hint: `On ${theOther}, open Tandem and press “Pair”.`,
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

/** Hay un emparejamiento vivo: cerrar el diálogo lo cancela (y avisa a the other machine). */
export function isActive(pairing: PairingView): boolean {
  return pairing.kind === "searching" || pairing.kind === "comparing";
}

/** Cerrar un resultado ya mostrado lo deja en reposo en el agente. */
export function needsDismiss(pairing: PairingView): boolean {
  return pairing.kind === "done" || pairing.kind === "failed";
}

/**
 * Abrir el diálogo solo: cuando el emparejamiento pasa a estar vivo (por ejemplo, la
 * ventana se abrió con uno in progress) o aparece el código. Nunca para un resultado viejo.
 */
export function shouldOpen(before: PairingView | null, after: PairingView): boolean {
  if (!isActive(after)) return false;
  if (before === null || !isActive(before)) return true;
  return before.kind === "searching" && after.kind === "comparing";
}

/** El texto del botón de la píldora del enlace. */
export function pairButton(peer: PeerView): string {
  return peer.kind === "paired" ? "Pair again" : "Pair";
}

// ---- Escribir el código (asistente, «Join a desk») -----------------------------------

/** Pone un dígito en su casilla (vacío la borra); cualquier otra cosa se ignora. */
export function setDigit(digits: string[], i: number, value: string): string[] {
  const d = value.replace(/\D/g, "").slice(-1);
  return digits.map((x, j) => (j === i ? d : x));
}

export const isComplete = (digits: string[]): boolean => digits.length === 6 && digits.every((d) => /^\d$/.test(d));

/** ¿Lo escrito es el código que calculó el agente? Solo cuenta con las seis casillas llenas. */
export function codeMatches(typed: string[], code: string): boolean {
  return isComplete(typed) && typed.join("") === code.replace(/\D/g, "");
}
