// The pairing screen from the peer and the progress the agent sends. Pure,
// to test without a DOM.

import type { PairingView, PeerView } from "./model";

export type PairScreen =
  /** No peer and not paired: what is going to happen and the button to start. */
  | { kind: "intro"; other: string }
  /** Already paired: with whom, and unpair. */
  | { kind: "paired"; name: string }
  /** Asked to unpair: confirmed first. */
  | { kind: "unpair"; name: string }
  | { kind: "searching"; other: string; hint: string }
  | { kind: "compare"; digits: [string, string]; peer: string; confirmed: boolean }
  | { kind: "done"; peer: string }
  | { kind: "failed"; reason: string };

/** The state of the line between the two machines that the dialog draws. */
export type PairLine = "idle" | "searching" | "found" | "linked" | "broken";

/**
 * What the dialog shows. `theOther` is the name of the other machine (from its announcement, or
 * "the other machine" if it is not known yet): nothing is assumed by system (D2).
 */
export function pairScreen(theOther: string, peer: PeerView, pairing: PairingView, askUnpair = false): PairScreen {
  switch (pairing.kind) {
    case "searching":
      return {
        kind: "searching",
        other: theOther,
        // One waits and the other searches for it: what the user has to do is the same.
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

/** 6 digits in two groups of 3, which compare better at a glance. */
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

/** There is a live pairing: closing the dialog cancels it (and notifies the other machine). */
export function isActive(pairing: PairingView): boolean {
  return pairing.kind === "searching" || pairing.kind === "comparing";
}

/** Closing an already shown result leaves it at rest in the agent. */
export function needsDismiss(pairing: PairingView): boolean {
  return pairing.kind === "done" || pairing.kind === "failed";
}

/**
 * Open the dialog by itself: when pairing becomes live (for example, the
 * window opened with one in progress) or the code appears. Never for an old result.
 */
export function shouldOpen(before: PairingView | null, after: PairingView): boolean {
  if (!isActive(after)) return false;
  if (before === null || !isActive(before)) return true;
  return before.kind === "searching" && after.kind === "comparing";
}

/** The text of the link pill button. */
export function pairButton(peer: PeerView): string {
  return peer.kind === "paired" ? "Pair again" : "Pair";
}

// ---- Typing the code (setup, "Join a desk") -------------------------------------------

/** Puts a digit in its box (empty clears it); anything else is ignored. */
export function setDigit(digits: string[], i: number, value: string): string[] {
  const d = value.replace(/\D/g, "").slice(-1);
  return digits.map((x, j) => (j === i ? d : x));
}

export const isComplete = (digits: string[]): boolean => digits.length === 6 && digits.every((d) => /^\d$/.test(d));

/** Is what was typed the code the agent computed? It only counts with all six boxes filled. */
export function codeMatches(typed: string[], code: string): boolean {
  return isComplete(typed) && typed.join("") === code.replace(/\D/g, "");
}
