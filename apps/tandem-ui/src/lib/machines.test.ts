import { describe, expect, it } from "vitest";

import type { Caps, Desk, MachineCaps } from "./model";
import { advance, capsOf, isRunning, machineColor, machineName, other, stepLabel, type Progress } from "./machines";

function machineCaps(machine: "a" | "b", extra: Partial<MachineCaps> = {}): MachineCaps {
  return {
    machine,
    name: machine === "a" ? "Estudio" : "Taller",
    os: "windows",
    color: machine === "a" ? "#6cc59a" : "#d9a75a",
    hub: null,
    input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 0, started: 0 } },
    power: { wake_peer: true, wakeable: true, power_off: true },
    services: [],
    ...extra,
  };
}

const desk: Desk = {
  machines: { a: { os: "macos", name: "Guardada A", color: "#111111" }, b: null },
  screens: [],
  arrangement: null,
};

describe("lados", () => {
  it("cada lado tiene al otro", () => {
    expect(other("a")).toBe("b");
    expect(other("b")).toBe("a");
  });

  it("los nombres salen del anuncio, del puesto o son «máquina A/B»; nunca Mac/PC fijos", () => {
    const caps: Caps = { here: machineCaps("b"), peer: null };
    expect(machineName("b", caps, desk)).toBe("Taller");
    expect(machineName("a", caps, desk)).toBe("Guardada A");
    expect(machineName("b", null, { ...desk, machines: { a: null, b: null } })).toBe("máquina B");
    expect(machineName("a")).toBe("máquina A");
  });

  it("el color sale del anuncio o del puesto, con uno por omisión por lado", () => {
    const caps: Caps = { here: machineCaps("a"), peer: machineCaps("b") };
    expect(machineColor("a", caps)).toBe("#6cc59a");
    expect(machineColor("a", null, desk)).toBe("#111111");
    expect(machineColor("b")).toBe("#a99be8");
  });

  it("dos Windows y dos Mac funcionan igual", () => {
    const caps: Caps = { here: machineCaps("a"), peer: machineCaps("b") };
    expect(capsOf(caps, "a")?.os).toBe("windows");
    expect(capsOf(caps, "b")?.os).toBe("windows");
    const macs: Caps = { here: machineCaps("a", { os: "macos" }), peer: machineCaps("b", { os: "macos" }) };
    expect([capsOf(macs, "a")?.os, capsOf(macs, "b")?.os]).toEqual(["macos", "macos"]);
    expect(capsOf({ here: machineCaps("a"), peer: null }, "b")).toBeNull();
  });
});

describe("progreso por pasos", () => {
  const name = (m: "a" | "b") => (m === "a" ? "Estudio" : "Taller");

  it("nombra cada paso con la máquina", () => {
    expect(stepLabel({ step: "power_on", machine: "b" }, name)).toBe("Encender Taller");
    expect(stepLabel({ step: "power_off", machine: "a" }, name)).toBe("Apagar Estudio");
  });

  it("avanza, termina y falla con el error del paso", () => {
    let p: Progress | null = advance(null, { type: "started", txn: 1, steps: [{ step: "power_on", machine: "b" }] });
    expect(isRunning(p)).toBe(true);
    p = advance(p, { type: "step_started", txn: 1, index: 0 });
    expect(p?.status).toEqual(["run"]);
    p = advance(p, { type: "step_done", txn: 1, index: 0 });
    p = advance(p, { type: "committed", txn: 1, state: { on: { a: true, b: true } } });
    expect(p?.outcome).toEqual({ kind: "committed" });
    expect(isRunning(p)).toBe(false);

    let f = advance(null, { type: "started", txn: 2, steps: [{ step: "power_off", machine: "a" }] });
    f = advance(f, { type: "step_failed", txn: 2, index: 0, error: "sin respuesta" });
    f = advance(f, { type: "failed", txn: 2, state: { on: { a: true, b: false } } });
    expect(f?.outcome).toEqual({ kind: "failed", error: "sin respuesta" });
  });

  it("ignora avisos de otra transacción", () => {
    const p = advance(null, { type: "started", txn: 3, steps: [] });
    expect(advance(p, { type: "committed", txn: 9, state: { on: { a: true, b: false } } })).toBe(p);
    expect(advance(null, { type: "committed", txn: 9, state: { on: { a: true, b: false } } })).toBeNull();
  });
});
