import { describe, expect, it } from "vitest";

import type { Caps, Desk, MachineCaps } from "./model";
import { advance, capsOf, isRunning, machineColor, machineName, other, stepLabel, type Progress } from "./machines";

function machineCaps(machine: "a" | "b", extra: Partial<MachineCaps> = {}): MachineCaps {
  return {
    machine,
    name: machine === "a" ? "Studio" : "Workshop",
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
  machines: { a: { os: "macos", name: "Saved A", color: "#111111" }, b: null },
  screens: [],
  arrangement: null,
  scenes: [],
};

describe("sides", () => {
  it("each side has the other", () => {
    expect(other("a")).toBe("b");
    expect(other("b")).toBe("a");
  });

  it("names come from the announcement, from the desk or are \"machine A/B\"; never fixed Mac/PC", () => {
    const caps: Caps = { here: machineCaps("b"), peer: null };
    expect(machineName("b", caps, desk)).toBe("Workshop");
    expect(machineName("a", caps, desk)).toBe("Saved A");
    expect(machineName("b", null, { ...desk, machines: { a: null, b: null } })).toBe("machine B");
    expect(machineName("a")).toBe("machine A");
  });

  it("the color comes from the announcement or the desk, with a default one per side", () => {
    const caps: Caps = { here: machineCaps("a"), peer: machineCaps("b") };
    expect(machineColor("a", caps)).toBe("#6cc59a");
    expect(machineColor("a", null, desk)).toBe("#111111");
    expect(machineColor("b")).toBe("#a99be8");
  });

  it("two Windows and two Macs work the same", () => {
    const caps: Caps = { here: machineCaps("a"), peer: machineCaps("b") };
    expect(capsOf(caps, "a")?.os).toBe("windows");
    expect(capsOf(caps, "b")?.os).toBe("windows");
    const macs: Caps = { here: machineCaps("a", { os: "macos" }), peer: machineCaps("b", { os: "macos" }) };
    expect([capsOf(macs, "a")?.os, capsOf(macs, "b")?.os]).toEqual(["macos", "macos"]);
    expect(capsOf({ here: machineCaps("a"), peer: null }, "b")).toBeNull();
  });
});

describe("progress by steps", () => {
  const name = (m: "a" | "b") => (m === "a" ? "Studio" : "Workshop");

  it("names each step with the machine", () => {
    expect(stepLabel({ step: "power_on", machine: "b" }, name)).toBe("Power on Workshop");
    expect(stepLabel({ step: "power_off", machine: "a" }, name)).toBe("Power off Studio");
    expect(stepLabel({ step: "show_machine", monitor: "34", machine: "b" }, name)).toBe("Monitor 34 → Workshop");
    expect(stepLabel({ step: "geometry" }, name)).toBe("Keyboard and mouse: new geometry");
  });

  it("advances, finishes and fails with the step's error", () => {
    let p: Progress | null = advance(null, { type: "started", txn: 1, steps: [{ step: "power_on", machine: "b" }] });
    expect(isRunning(p)).toBe(true);
    p = advance(p, { type: "step_started", txn: 1, index: 0 });
    expect(p?.status).toEqual(["run"]);
    p = advance(p, { type: "step_done", txn: 1, index: 0 });
    p = advance(p, { type: "committed", txn: 1, state: { on: { a: true, b: true } } });
    expect(p?.outcome).toEqual({ kind: "committed" });
    expect(isRunning(p)).toBe(false);

    let f = advance(null, { type: "started", txn: 2, steps: [{ step: "power_off", machine: "a" }] });
    f = advance(f, { type: "step_failed", txn: 2, index: 0, error: "no reply" });
    f = advance(f, { type: "failed", txn: 2, state: { on: { a: true, b: false } } });
    expect(f?.outcome).toEqual({ kind: "failed", error: "no reply" });

    // A scene that fails on the second monitor undoes the first.
    const show = (monitor: string) => ({ step: "show_machine" as const, monitor, machine: "b" as const });
    let u = advance(null, { type: "started", txn: 4, steps: [show("34"), show("27")] });
    u = advance(u, { type: "step_done", txn: 4, index: 0 });
    u = advance(u, { type: "step_failed", txn: 4, index: 1, error: "no verdict" });
    u = advance(u, { type: "step_undone", txn: 4, index: 0 });
    expect(u?.status).toEqual(["undone", "failed"]);
  });

  it("ignores notices of another transaction", () => {
    const p = advance(null, { type: "started", txn: 3, steps: [] });
    expect(advance(p, { type: "committed", txn: 9, state: { on: { a: true, b: false } } })).toBe(p);
    expect(advance(null, { type: "committed", txn: 9, state: { on: { a: true, b: false } } })).toBeNull();
  });
});
