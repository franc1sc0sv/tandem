import { describe, expect, it } from "vitest";

import {
  STEPS,
  advanceTo,
  begin,
  canAdvance,
  countsAsKeyProbe,
  goBack,
  nextStep,
  shouldOpen,
  summaryChecks,
  viewOf,
  type Onboarding,
  type StepId,
} from "./onboarding";
import type { InputView, MachineCaps, PairingView, PeerView, Snapshot } from "./model";

const PROCESS = { exe: "/Applications/Tandem.app/Contents/MacOS/tandemd", pid: 4242, started: 1 };

// Datos de ejemplo: A con macOS y B con Windows (podría ser cualquier combinación, D2).
function caps(machine: "a" | "b", extra: Partial<MachineCaps> = {}): MachineCaps {
  const pc = machine === "b";
  return {
    machine,
    name: pc ? "ESCRITORIO" : "MacBook-Pro",
    os: pc ? "windows" : "macos",
    color: pc ? "#a99be8" : "#7aa7e6",
    hub: null,
    input: { capture: true, inject: true, missing: [], process: PROCESS },
    power: { wake_peer: true, wakeable: true, power_off: pc },
    services: [],
    ...extra,
  };
}

const INPUT: InputView = { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: false };
const PAIRED: PeerView = { kind: "paired", name: "ESCRITORIO", link: "usb4" };
const IDLE: PairingView = { kind: "idle" };

interface Parts {
  agent: "a" | "b";
  peer: PeerView;
  pairing: PairingView;
  here: MachineCaps;
  input: InputView;
}

function snap(over: Partial<Parts> = {}): Snapshot {
  const agent = over.agent ?? "a";
  return {
    kind: "connected",
    agent,
    desk: { machines: { a: null, b: null }, screens: [], arrangement: null, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: over.peer ?? PAIRED,
    pairing: over.pairing ?? IDLE,
    input: over.input ?? INPUT,
    caps: { here: over.here ?? caps(agent), peer: caps(agent === "a" ? "b" : "a") },
    monitors: [],
  };
}

function at(step: StepId, patch: Partial<Onboarding> = {}): Onboarding {
  return { ...begin(step), ...patch };
}

const ids = (v: { buttons: { id: string }[] }) => v.buttons.map((b) => b.id);
const btn = (v: { buttons: { id: string; enabled: boolean }[] }, id: string) => v.buttons.find((b) => b.id === id);

// ---------------------------------------------------------------------------------------

describe("los pasos", () => {
  it("son cuatro, sin monitores, y en este orden (DESIGN §5)", () => {
    expect(STEPS.map((s) => s.id)).toEqual(["machine", "pair", "keyboard", "summary"]);
  });

  it("se abre solo en una instalación limpia: sin par y sin teclado dicho", () => {
    expect(shouldOpen(snap({ peer: { kind: "unpaired" } }))).toBe(true);
    expect(shouldOpen(snap())).toBe(false);
    expect(shouldOpen(snap({ peer: { kind: "unpaired" }, input: { ...INPUT, defined: true } }))).toBe(false);
    expect(shouldOpen({ kind: "connecting" })).toBe(false);
  });
});

describe("paso 1: this machine", () => {
  it("dice cómo se detectó la máquina", () => {
    const v = viewOf(at("machine"), snap({ agent: "b" }));
    expect(v.checks[0]).toMatchObject({ state: "ok", label: "ESCRITORIO", detail: expect.stringContaining("Windows") });
    expect(v.verified).toBe(true);
  });

  it("en macOS verifica los permisos en vivo: mientras falte uno no se avanza", () => {
    const missing = caps("a", { input: { capture: false, inject: false, missing: ["accessibility", "input_monitoring"], process: PROCESS } });
    const v = viewOf(at("machine"), snap({ here: missing }));
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("macOS permissions are missing");
    expect(ids(v)).toEqual(["perm:request", "perm:accessibility", "perm:input_monitoring", "perm:restart"]);
    expect(v.checks.filter((c) => c.state === "fail")).toHaveLength(2);
    // Los nombres salen también como los llama macOS en inglés.
    expect(v.checks.some((c) => c.label === "Permiso: Accesibilidad (Accessibility)")).toBe(true);
    expect(v.checks.some((c) => c.label === "Permiso: Input Monitoring")).toBe(true);
    // Y a qué binario concederlos, sin abrir una terminal.
    const binary = v.checks.find((c) => c.label === "Agent binary");
    expect(binary?.detail).toContain("/Applications/Tandem.app/Contents/MacOS/tandemd");
    expect(v.checks.some((c) => c.label === "How to grant" && (c.detail ?? "").includes("Restart agent"))).toBe(true);
    // Se concede uno: el otro sigue.
    const one = caps("a", { input: { capture: true, inject: true, missing: ["input_monitoring"], process: PROCESS } });
    expect(ids(viewOf(at("machine"), snap({ here: one })))).toEqual(["perm:request", "perm:input_monitoring", "perm:restart"]);
    // Se conceden todos: se comprueba solo.
    const ok = viewOf(at("machine"), snap());
    expect(ok.verified).toBe(true);
    expect(ok.checks.some((c) => c.label === "macOS permissions" && c.state === "ok")).toBe(true);
    // Con todo concedido no se ofrece pedir ni reiniciar nada.
    expect(ids(ok)).toEqual([]);
  });

  it("sin agente no hay nada que verificar", () => {
    const v = viewOf(at("machine"), { kind: "connecting" });
    expect(v.verified).toBe(false);
    expect(v.blocker).toContain("tandemd");
  });
});

describe("paso 2: the other machine", () => {
  const unpaired: PeerView = { kind: "unpaired" };

  it("sin emparejar se puede buscar o seguir solo por ahora", () => {
    const v = viewOf(at("pair"), snap({ peer: unpaired }));
    expect(ids(v)).toEqual(["pair:start", "pair:skip"]);
    expect(v.verified).toBe(false);
    const skipped = viewOf(at("pair", { pair: { ...begin().pair, skipped: true } }), snap({ peer: unpaired }));
    expect(skipped.verified).toBe(true);
    expect(skipped.checks.some((c) => c.label.includes("solo con this machine"))).toBe(true);
  });

  it("muestra el código y pide compararlo; no se puede saltar a mitad", () => {
    const comparing: PairingView = { kind: "comparing", code: "418027", peer: "ESCRITORIO", confirmed: false };
    const v = viewOf(at("pair"), snap({ peer: unpaired, pairing: comparing }));
    expect(v.code).toBe("418027");
    expect(ids(v)).toEqual(["pair:confirm", "pair:cancel", "pair:skip"]);
    expect(btn(v, "pair:skip")?.enabled).toBe(false);
    const waiting = viewOf(at("pair"), snap({ peer: unpaired, pairing: { ...comparing, confirmed: true } }));
    expect(ids(waiting)).not.toContain("pair:confirm");
    const searching = viewOf(at("pair"), snap({ peer: unpaired, pairing: { kind: "searching" } }));
    expect(ids(searching)).toContain("pair:cancel");
  });

  it("un emparejamiento fallido dice por qué", () => {
    const v = viewOf(at("pair"), snap({ peer: unpaired, pairing: { kind: "failed", reason: "se rechazó en ESCRITORIO" } }));
    expect(v.checks[0]).toMatchObject({ state: "fail", detail: "se rechazó en ESCRITORIO" });
  });

  it("emparejada pero sin verificar el enlace no avanza: hace falta el ping", () => {
    const v = viewOf(at("pair"), snap());
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("test the link");
    expect(btn(v, "ping")).toMatchObject({ enabled: true, label: "Test the link" });
    const done = viewOf(at("pair", { pair: { ...begin().pair, ping: { link: "wifi", millis: 7 } } }), snap());
    expect(done.verified).toBe(true);
    expect(done.checks.find((c) => c.label === "Link ping")).toMatchObject({ state: "ok", detail: "Wi-Fi · 7 ms" });
  });

  it("emparejada pero no link no se puede verificar", () => {
    const v = viewOf(at("pair", { pair: { ...begin().pair, ping: { link: "wifi", millis: 7 } } }), snap({ peer: { kind: "paired", name: "ESCRITORIO", link: null } }));
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("the other machine does not respond");
  });

  it("un ping fallido se ve", () => {
    const v = viewOf(at("pair", { pair: { ...begin().pair, pingError: "ningún enlace ve a the other machine" } }), snap());
    expect(v.checks.find((c) => c.label === "Link ping")).toMatchObject({ state: "fail" });
  });
});

describe("paso 3: teclado y mouse", () => {
  it("al principio no hay nada definido y a machine is never assumed", () => {
    const v = viewOf(at("keyboard"), snap());
    expect(v.verified).toBe(false);
    expect(ids(v)).toEqual(["key:start", "key:pick:a", "key:pick:b", "key:skip"]);
    expect(v.checks.some((c) => c.detail === "a machine is never assumed")).toBe(true);
  });

  it("escucha en las dos máquinas mientras espera la tecla", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, armed: true } }), snap());
    expect(v.checks[0]).toMatchObject({ state: "wait" });
    expect(v.checks[0]!.label).toContain("las dos máquinas");
    expect(ids(v)).not.toContain("key:start");
  });

  it("la máquina que recibió la tecla la tiene, pero hay que confirmarlo", () => {
    const heard = at("keyboard", { keyboard: { ...begin().keyboard, armed: true, heard: "b" } });
    const v = viewOf(heard, snap());
    expect(v.checks[0]!.label).toBe("La tecla llegó a ESCRITORIO");
    expect(ids(v)).toEqual(["key:yes:b", "key:yes:a", "key:again"]);
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("confirm where the keyboard is");
  });

  it("confirmado queda verificado", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, confirmed: "b" } }), snap());
    expect(v.verified).toBe(true);
    expect(v.checks[0]).toMatchObject({ state: "ok", label: "El teclado y el mouse están en ESCRITORIO" });
  });

  it("sin respuesta se deja explícitamente sin definir", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, undefinedOk: true } }), snap());
    expect(v.verified).toBe(true);
    expect(v.checks.some((c) => c.label === "Undefined")).toBe(true);
  });

  it("sin the other machine enlazada avisa que solo se escucha aquí", () => {
    const v = viewOf(at("keyboard"), snap({ peer: { kind: "unpaired" } }));
    expect(v.checks[0]!.detail).toContain("solo se escucha aquí");
  });
});

describe("paso 4: resumen", () => {
  it("junta todo lo verificado, con el nombre de cada máquina", () => {
    const o = at("summary", {
      pair: { ...begin().pair, ping: { link: "usb4", millis: 2 } },
      keyboard: { ...begin().keyboard, confirmed: "b" },
    });
    const text = summaryChecks(o, snap()).map((l) => `${l.state}|${l.label}|${l.detail ?? ""}`);
    expect(text).toEqual([
      "ok|MacBook-Pro|permissions verified",
      "ok|ESCRITORIO emparejada|USB-C · 2 ms",
      "ok|Keyboard and mouse en ESCRITORIO|confirmed by you",
    ]);
    expect(viewOf(o, snap()).buttons.map((b) => b.id)).toEqual(["finish"]);
  });

  it("lo que no se hizo se dice: teclado sin definir, sin the other machine", () => {
    const lines = summaryChecks(begin("summary"), snap({ peer: { kind: "unpaired" } })).map((l) => l.label);
    expect(lines).toContain("Keyboard and mouse undefined");
    expect(lines).toContain("Without the other machine");
  });
});

describe("navegación", () => {
  it("no avanza si el paso no está verificado", () => {
    const o = at("pair");
    expect(canAdvance(o, snap())).toBe(false);
    expect(advanceTo(o, snap())).toBe(o);
    const ok = at("pair", { pair: { ...begin().pair, ping: { link: "usb4", millis: 1 } } });
    expect(canAdvance(ok, snap())).toBe(true);
    expect(advanceTo(ok, snap()).step).toBe("keyboard");
  });

  it("se puede volver atrás hasta el primero, y en el resumen ya no se avanza", () => {
    expect(goBack(at("machine")).step).toBe("machine");
    expect(goBack(at("keyboard")).step).toBe("pair");
    expect(goBack(at("summary")).step).toBe("keyboard");
    expect(nextStep(at("keyboard"))).toBe("summary");
    expect(canAdvance(at("summary"), snap())).toBe(false);
  });

  it("un recorrido completo pasa por los cuatro pasos", () => {
    let o = begin();
    const visited: StepId[] = [o.step];
    const facts: Record<StepId, Partial<Onboarding>> = {
      machine: {},
      pair: { pair: { ...begin().pair, ping: { link: "usb4", millis: 3 } } },
      keyboard: { keyboard: { ...begin().keyboard, confirmed: "b" } },
      summary: {},
    };
    for (let i = 0; i < 3; i++) {
      o = { ...o, ...facts[o.step] };
      o = advanceTo(o, snap());
      visited.push(o.step);
    }
    expect(visited).toEqual(["machine", "pair", "keyboard", "summary"]);
  });
});

describe("presionar una tecla", () => {
  it("cuenta cualquier tecla del teclado, pero no activar un botón del diálogo", () => {
    expect(countsAsKeyProbe({ key: "a", targetIsButton: false })).toBe(true);
    expect(countsAsKeyProbe({ key: "a", targetIsButton: true })).toBe(true);
    expect(countsAsKeyProbe({ key: "Enter", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: " ", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: "Tab", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: "Enter", targetIsButton: false })).toBe(true);
    expect(countsAsKeyProbe({ key: "Shift", targetIsButton: false })).toBe(false);
    expect(countsAsKeyProbe({ key: "a", repeat: true, targetIsButton: false })).toBe(false);
  });
});
