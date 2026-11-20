import { describe, expect, it } from "vitest";

import type { PairingView, PeerView } from "./model";
import { codeMatches, isActive, isComplete, needsDismiss, pairButton, pairLine, pairScreen, setDigit, shouldOpen, splitCode } from "./pairing";

const UNPAIRED: PeerView = { kind: "unpaired" };
const PAIRED: PeerView = { kind: "paired", name: "Workshop", link: "usb4" };
const IDLE: PairingView = { kind: "idle" };
const SEARCHING: PairingView = { kind: "searching" };
const COMPARING: PairingView = { kind: "comparing", code: "042917", peer: "Workshop", confirmed: false };

describe("screen", () => {
  it("without a peer offers to pair with the other machine", () => {
    expect(pairScreen("Workshop", UNPAIRED, IDLE)).toEqual({ kind: "intro", other: "Workshop" });
    expect(pairScreen("Studio", UNPAIRED, IDLE)).toEqual({ kind: "intro", other: "Studio" });
  });

  it("paired shows with whom and asks to confirm before unpairing", () => {
    expect(pairScreen("Workshop", PAIRED, IDLE)).toEqual({ kind: "paired", name: "Workshop" });
    expect(pairScreen("Workshop", PAIRED, IDLE, true)).toEqual({ kind: "unpair", name: "Workshop" });
    // Without a peer there is nothing to unpair.
    expect(pairScreen("Workshop", UNPAIRED, IDLE, true).kind).toBe("intro");
  });

  it("a pairing offer from another machine leaves the screen as if idle: the paired peer is still the pair (T-345)", () => {
    const OFFERED: PairingView = { kind: "offered" };
    expect(pairScreen("Workshop", PAIRED, OFFERED)).toEqual({ kind: "paired", name: "Workshop" });
    expect(pairScreen("Workshop", UNPAIRED, OFFERED)).toEqual({ kind: "intro", other: "Workshop" });
    // It is not a live pairing: closing the dialog has nothing to cancel.
    expect(isActive(OFFERED)).toBe(false);
    expect(needsDismiss(OFFERED)).toBe(false);
    expect(shouldOpen(null, OFFERED)).toBe(false);
  });

  it("searching says what to do on the other machine", () => {
    const s = pairScreen("Workshop", UNPAIRED, SEARCHING);
    expect(s).toMatchObject({ kind: "searching", other: "Workshop" });
    expect(s.kind === "searching" && s.hint).toContain("On Workshop");
    expect(pairScreen("Studio", UNPAIRED, SEARCHING)).toMatchObject({ other: "Studio" });
  });

  it("the code is shown the same on both machines, in two groups", () => {
    const a = pairScreen("Workshop", UNPAIRED, COMPARING);
    const b = pairScreen("Studio", UNPAIRED, { ...COMPARING, peer: "Studio" });
    expect(a).toEqual({ kind: "compare", digits: ["042", "917"], peer: "Workshop", confirmed: false });
    expect(b.kind === "compare" && b.digits).toEqual(["042", "917"]);
    expect(pairScreen("Workshop", UNPAIRED, { ...COMPARING, confirmed: true })).toMatchObject({ confirmed: true });
  });

  it("the result takes precedence over the peer", () => {
    expect(pairScreen("Workshop", PAIRED, { kind: "done", peer: "Workshop" })).toEqual({ kind: "done", peer: "Workshop" });
    expect(pairScreen("Workshop", UNPAIRED, { kind: "failed", reason: "rejected on Workshop" })).toEqual({
      kind: "failed",
      reason: "rejected on Workshop",
    });
  });
});

describe("code", () => {
  it("it is always 6 digits with the zeros", () => {
    expect(splitCode("000042")).toEqual(["000", "042"]);
    expect(splitCode("42")).toEqual(["000", "042"]);
    expect(splitCode("123 456")).toEqual(["123", "456"]);
  });
});

describe("line between the machines", () => {
  it("follows the progress", () => {
    const line = (p: PairingView, peer: PeerView = UNPAIRED, ask = false) => pairLine(pairScreen("Workshop", peer, p, ask));
    expect(line(IDLE)).toBe("idle");
    expect(line(SEARCHING)).toBe("searching");
    expect(line(COMPARING)).toBe("found");
    expect(line({ kind: "done", peer: "Workshop" })).toBe("linked");
    expect(line(IDLE, PAIRED)).toBe("linked");
    expect(line(IDLE, PAIRED, true)).toBe("broken");
    expect(line({ kind: "failed", reason: "x" })).toBe("broken");
  });
});

describe("open and close", () => {
  it("opens by itself when starting or when the code appears, not with an old result", () => {
    expect(shouldOpen(null, SEARCHING)).toBe(true);
    expect(shouldOpen(IDLE, SEARCHING)).toBe(true);
    expect(shouldOpen(SEARCHING, COMPARING)).toBe(true);
    expect(shouldOpen(null, COMPARING)).toBe(true);
    expect(shouldOpen(SEARCHING, SEARCHING)).toBe(false);
    expect(shouldOpen(COMPARING, { ...COMPARING, confirmed: true })).toBe(false);
    expect(shouldOpen(null, { kind: "done", peer: "Workshop" })).toBe(false);
    expect(shouldOpen(null, { kind: "failed", reason: "x" })).toBe(false);
    expect(shouldOpen(null, IDLE)).toBe(false);
  });

  it("closing cancels what is live and discards what is finished", () => {
    expect(isActive(SEARCHING) && isActive(COMPARING)).toBe(true);
    expect(isActive(IDLE) || isActive({ kind: "done", peer: "Workshop" })).toBe(false);
    expect(needsDismiss({ kind: "done", peer: "Workshop" }) && needsDismiss({ kind: "failed", reason: "x" })).toBe(true);
    expect(needsDismiss(IDLE) || needsDismiss(SEARCHING)).toBe(false);
  });

  it("the pill offers to pair if there is no peer", () => {
    expect(pairButton(UNPAIRED)).toBe("Pair");
    expect(pairButton(PAIRED)).toBe("Pair again");
  });
});

describe("typing the code", () => {
  it("each box holds one digit and clears when emptied", () => {
    expect(setDigit(["", "", "", "", "", ""], 1, "7")).toEqual(["", "7", "", "", "", ""]);
    expect(setDigit(["1", "2", "", "", "", ""], 1, "")).toEqual(["1", "", "", "", "", ""]);
    expect(setDigit(["", "", "", "", "", ""], 0, "x")).toEqual(["", "", "", "", "", ""]);
  });

  it("it only matches with all six boxes filled and the same code", () => {
    const full = ["4", "1", "8", "0", "2", "7"];
    expect(isComplete(full)).toBe(true);
    expect(isComplete(["4", "1", "8", "0", "2", ""])).toBe(false);
    expect(codeMatches(full, "418027")).toBe(true);
    expect(codeMatches(full, "418 027")).toBe(true);
    expect(codeMatches(["4", "1", "8", "0", "2", "8"], "418027")).toBe(false);
    expect(codeMatches(["4", "1", "8", "0", "2", ""], "418027")).toBe(false);
  });
});
