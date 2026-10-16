import { describe, expect, it } from "vitest";

import type { PairingView, PeerView } from "./model";
import { isActive, needsDismiss, pairButton, pairLine, pairScreen, shouldOpen, splitCode } from "./pairing";

const UNPAIRED: PeerView = { kind: "unpaired" };
const PAIRED: PeerView = { kind: "paired", name: "PC", link: "usb4" };
const IDLE: PairingView = { kind: "idle" };
const SEARCHING: PairingView = { kind: "searching" };
const COMPARING: PairingView = { kind: "comparing", code: "042917", peer: "PC", confirmed: false };

describe("pantalla", () => {
  it("sin par ofrece emparejar con la otra máquina", () => {
    expect(pairScreen("mac", UNPAIRED, IDLE)).toEqual({ kind: "intro", other: "PC" });
    expect(pairScreen("pc", UNPAIRED, IDLE)).toEqual({ kind: "intro", other: "Mac" });
  });

  it("emparejado muestra con quién y pide confirmar antes de desemparejar", () => {
    expect(pairScreen("mac", PAIRED, IDLE)).toEqual({ kind: "paired", name: "PC" });
    expect(pairScreen("mac", PAIRED, IDLE, true)).toEqual({ kind: "unpair", name: "PC" });
    // Sin par no hay nada que desemparejar.
    expect(pairScreen("mac", UNPAIRED, IDLE, true).kind).toBe("intro");
  });

  it("buscando dice qué hacer en la otra máquina", () => {
    const s = pairScreen("mac", UNPAIRED, SEARCHING);
    expect(s).toMatchObject({ kind: "searching", other: "PC" });
    expect(s.kind === "searching" && s.hint).toContain("En la PC");
    expect(pairScreen("pc", UNPAIRED, SEARCHING)).toMatchObject({ other: "Mac" });
  });

  it("el código se muestra igual en las dos máquinas, en dos grupos", () => {
    const mac = pairScreen("mac", UNPAIRED, COMPARING);
    const pc = pairScreen("pc", UNPAIRED, { ...COMPARING, peer: "Mac" });
    expect(mac).toEqual({ kind: "compare", digits: ["042", "917"], peer: "PC", confirmed: false });
    expect(pc.kind === "compare" && pc.digits).toEqual(["042", "917"]);
    expect(pairScreen("mac", UNPAIRED, { ...COMPARING, confirmed: true })).toMatchObject({ confirmed: true });
  });

  it("el resultado manda sobre el par", () => {
    expect(pairScreen("mac", PAIRED, { kind: "done", peer: "PC" })).toEqual({ kind: "done", peer: "PC" });
    expect(pairScreen("mac", UNPAIRED, { kind: "failed", reason: "se rechazó en la PC" })).toEqual({
      kind: "failed",
      reason: "se rechazó en la PC",
    });
  });
});

describe("código", () => {
  it("siempre son 6 dígitos con los ceros", () => {
    expect(splitCode("000042")).toEqual(["000", "042"]);
    expect(splitCode("42")).toEqual(["000", "042"]);
    expect(splitCode("123 456")).toEqual(["123", "456"]);
  });
});

describe("línea entre las máquinas", () => {
  it("sigue el avance", () => {
    const line = (p: PairingView, peer: PeerView = UNPAIRED, ask = false) => pairLine(pairScreen("mac", peer, p, ask));
    expect(line(IDLE)).toBe("idle");
    expect(line(SEARCHING)).toBe("searching");
    expect(line(COMPARING)).toBe("found");
    expect(line({ kind: "done", peer: "PC" })).toBe("linked");
    expect(line(IDLE, PAIRED)).toBe("linked");
    expect(line(IDLE, PAIRED, true)).toBe("broken");
    expect(line({ kind: "failed", reason: "x" })).toBe("broken");
  });
});

describe("abrir y cerrar", () => {
  it("se abre solo al empezar o al aparecer el código, no con un resultado viejo", () => {
    expect(shouldOpen(null, SEARCHING)).toBe(true);
    expect(shouldOpen(IDLE, SEARCHING)).toBe(true);
    expect(shouldOpen(SEARCHING, COMPARING)).toBe(true);
    expect(shouldOpen(null, COMPARING)).toBe(true);
    expect(shouldOpen(SEARCHING, SEARCHING)).toBe(false);
    expect(shouldOpen(COMPARING, { ...COMPARING, confirmed: true })).toBe(false);
    expect(shouldOpen(null, { kind: "done", peer: "PC" })).toBe(false);
    expect(shouldOpen(null, { kind: "failed", reason: "x" })).toBe(false);
    expect(shouldOpen(null, IDLE)).toBe(false);
  });

  it("cerrar cancela lo vivo y descarta lo terminado", () => {
    expect(isActive(SEARCHING) && isActive(COMPARING)).toBe(true);
    expect(isActive(IDLE) || isActive({ kind: "done", peer: "PC" })).toBe(false);
    expect(needsDismiss({ kind: "done", peer: "PC" }) && needsDismiss({ kind: "failed", reason: "x" })).toBe(true);
    expect(needsDismiss(IDLE) || needsDismiss(SEARCHING)).toBe(false);
  });

  it("la píldora ofrece emparejar si no hay par", () => {
    expect(pairButton(UNPAIRED)).toBe("Emparejar");
    expect(pairButton(PAIRED)).toBe("Par");
  });
});
