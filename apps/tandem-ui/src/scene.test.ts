import { describe, expect, it } from "vitest";

import type { Desk, DeskState, Step } from "./model";
import {
  advance,
  builtinOf,
  groupStatus,
  groupSteps,
  isRunning,
  isWide,
  modeOf,
  modeRequest,
  monitorsOf,
  pcPower,
  type Progress,
} from "./scene";

const desk: Desk = {
  monitors: [
    { id: "m34", label: '34"', wiring: { pc: "displayport", mac: "hdmi" } },
    { id: "m32", label: '32"', wiring: { pc: "displayport", mac: "hdmi" } },
  ],
};

function state(m34: "mac" | "pc", m32: "mac" | "pc", extra: Partial<DeskState> = {}): DeskState {
  return { owner: { m34, m32 }, pc_on: m34 === "pc" || m32 === "pc", builtin_pref: "off", ...extra };
}

describe("modos", () => {
  it("se deducen del reparto", () => {
    expect(modeOf(desk, state("mac", "mac"))).toBe("mac");
    expect(modeOf(desk, state("pc", "pc"))).toBe("pc");
    expect(modeOf(desk, state("pc", "mac"))).toBe("shared");
    expect(modeOf({ monitors: [] }, state("mac", "mac"))).toBeNull();
  });

  it("un monitor sin dueño guardado está en la Mac", () => {
    expect(modeOf(desk, { owner: {}, pc_on: false, builtin_pref: "off" })).toBe("mac");
  });

  it("compartido manda el monitor de la izquierda a la PC", () => {
    expect(modeRequest("shared", desk, state("mac", "mac"))).toEqual({
      mode: "shared",
      assign: { m34: "pc", m32: "mac" },
    });
    expect(modeRequest("shared", desk, state("pc", "pc"))).toEqual({
      mode: "shared",
      assign: { m34: "pc", m32: "mac" },
    });
  });

  it("compartido estando ya en compartido conserva el reparto", () => {
    expect(modeRequest("shared", desk, state("mac", "pc"))).toEqual({
      mode: "shared",
      assign: { m34: "mac", m32: "pc" },
    });
  });

  it("los modos solo no llevan reparto", () => {
    expect(modeRequest("mac", desk, state("pc", "pc"))).toEqual({ mode: "solo_mac" });
    expect(modeRequest("pc", desk, state("mac", "mac"))).toEqual({ mode: "solo_pc" });
  });
});

describe("monitores", () => {
  it("cada máquina muestra los suyos, en el orden del escritorio, con su entrada", () => {
    const s = state("pc", "mac");
    expect(monitorsOf(desk, s, "pc")).toEqual([
      { id: "m34", label: '34"', owner: "pc", port: "DP", wide: true, movable: true },
    ]);
    expect(monitorsOf(desk, s, "mac").map((m) => [m.id, m.port, m.wide])).toEqual([["m32", "HDMI", false]]);
  });

  it("un monitor cableado a una sola máquina no se puede mover", () => {
    const solo: Desk = { monitors: [{ id: "x", label: "TV", wiring: { mac: "hdmi" } }] };
    expect(monitorsOf(solo, { owner: { x: "mac" }, pc_on: false, builtin_pref: "off" }, "mac")[0].movable).toBe(false);
  });

  it("los ultrawide son los de 34\" o más", () => {
    expect(isWide({ id: "a", label: '34"', wiring: {} })).toBe(true);
    expect(isWide({ id: "a", label: '49" G9', wiring: {} })).toBe(true);
    expect(isWide({ id: "a", label: '32"', wiring: {} })).toBe(false);
    expect(isWide({ id: "a", label: "principal", wiring: {} })).toBe(false);
  });
});

describe("pantalla integrada", () => {
  it("sigue la preferencia mientras haya monitores en la Mac", () => {
    expect(builtinOf(desk, state("pc", "mac", { builtin_pref: "mirror" }))).toEqual({
      effective: "mirror",
      forced: false,
      note: "espejo",
    });
  });

  it("sin monitores en la Mac se enciende sola", () => {
    expect(builtinOf(desk, state("pc", "pc", { builtin_pref: "off" }))).toEqual({
      effective: "extend",
      forced: true,
      note: "encendida: no hay monitores",
    });
  });
});

const toPc: Step[] = [
  { step: "power_on_pc" },
  { step: "switch_input", monitor: "m34", from: "mac", to: "pc", input: "displayport" },
  { step: "attach", machine: "pc", monitor: "m34" },
  { step: "detach", machine: "mac", monitor: "m34" },
  { step: "switch_input", monitor: "m32", from: "mac", to: "pc", input: "displayport" },
  { step: "attach", machine: "pc", monitor: "m32" },
  { step: "detach", machine: "mac", monitor: "m32" },
  { step: "set_builtin", from: "off", to: "extend" },
  { step: "update_input_geometry" },
];

describe("progreso por pasos", () => {
  it("agrupa los pasos de cada monitor en uno", () => {
    expect(groupSteps(desk, toPc)).toEqual([
      { label: "Encender la PC", steps: [0] },
      { label: '34" → DP', steps: [1, 2, 3] },
      { label: '32" → DP', steps: [4, 5, 6] },
      { label: "Pantalla Mac encendida", steps: [7] },
      { label: "Teclado y mouse", steps: [8] },
    ]);
  });

  it("nombra el apagado y usa el id si el monitor no está en el puesto", () => {
    expect(
      groupSteps(desk, [
        { step: "switch_input", monitor: "otro", from: "pc", to: "mac", input: "hdmi" },
        { step: "power_off_pc" },
      ]).map((g) => g.label),
    ).toEqual(["otro → HDMI", "Apagado limpio"]);
  });

  function run(events: Parameters<typeof advance>[1][]): Progress | null {
    return events.reduce<Progress | null>((p, e) => advance(p, e), null);
  }

  it("sigue cada paso hasta confirmar", () => {
    const p = run([
      { type: "started", txn: 1, steps: toPc },
      { type: "step_started", txn: 1, index: 0 },
    ])!;
    expect(p.status[0]).toBe("run");
    expect(isRunning(p)).toBe(true);
    expect(pcPower(state("mac", "mac", { pc_on: false }), p)).toBe("boot");

    const later = run([
      { type: "started", txn: 1, steps: toPc },
      { type: "step_started", txn: 1, index: 0 },
      { type: "step_done", txn: 1, index: 0 },
      { type: "step_started", txn: 1, index: 1 },
      { type: "step_done", txn: 1, index: 1 },
    ])!;
    const groups = groupSteps(desk, toPc);
    expect(groups.map((g) => groupStatus(later, g))).toEqual(["done", "run", "pending", "pending", "pending"]);
    expect(pcPower(state("mac", "mac", { pc_on: true }), later)).toBe("on");

    const done = advance(later, { type: "committed", txn: 1, state: state("pc", "pc") })!;
    expect(done.outcome).toEqual({ kind: "committed" });
    expect(isRunning(done)).toBe(false);
  });

  it("un fallo se ve en su paso y se recuerda al deshacer", () => {
    const p = run([
      { type: "started", txn: 2, steps: toPc },
      { type: "step_started", txn: 2, index: 0 },
      { type: "step_failed", txn: 2, index: 0, error: "la PC no despertó" },
      { type: "rolling_back", txn: 2 },
    ])!;
    expect(groupStatus(p, groupSteps(desk, toPc)[0])).toBe("failed");
    expect(p.outcome).toEqual({ kind: "rolling_back", error: "la PC no despertó" });
    expect(isRunning(p)).toBe(true);
    expect(pcPower(state("mac", "mac", { pc_on: false }), p)).toBe("off");
    const back = advance(p, { type: "rolled_back", txn: 2, state: state("mac", "mac") })!;
    expect(back.outcome).toEqual({ kind: "rolled_back", error: "la PC no despertó" });
  });

  it("si no se pudo deshacer, lo dice", () => {
    const p = run([
      { type: "started", txn: 3, steps: toPc },
      { type: "step_failed", txn: 3, index: 1, error: "DDC no responde" },
      {
        type: "inconsistent",
        txn: 3,
        undo_failures: [{ step: { step: "update_input_geometry" }, error: "sin respuesta" }],
      },
    ])!;
    expect(p.outcome).toEqual({ kind: "inconsistent", error: "DDC no responde; al deshacer: sin respuesta" });
  });

  it("ignora los avisos de otra transacción", () => {
    const p = run([{ type: "started", txn: 4, steps: toPc }]);
    expect(advance(p, { type: "step_started", txn: 9, index: 0 })).toBe(p);
    expect(advance(null, { type: "step_started", txn: 9, index: 0 })).toBeNull();
  });

  it("una transacción vacía termina sin pasos", () => {
    const p = run([
      { type: "started", txn: 5, steps: [] },
      { type: "committed", txn: 5, state: state("mac", "mac") },
    ])!;
    expect(groupSteps(desk, p.steps)).toEqual([]);
    expect(isRunning(p)).toBe(false);
  });
});
