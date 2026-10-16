import { describe, expect, it } from "vitest";

import type { Caps, Desk, DeskState, MachineCaps, Step } from "./model";
import {
  advance,
  builtinOf,
  groupStatus,
  groupSteps,
  isRunning,
  isWide,
  modeOf,
  modeRequest,
  canSwitch,
  hasBuiltin,
  machineState,
  monitorsOf,
  peerPill,
  peerPower,
  powerButton,
  roleChip,
  sides,
  type Progress,
} from "./scene";
import type { InputView } from "./model";

const desk: Desk = {
  monitors: [
    { id: "m34", label: '34"', wiring: { pc: "displayport", mac: "hdmi" } },
    { id: "m32", label: '32"', wiring: { pc: "displayport", mac: "hdmi" } },
  ],
};

function state(m34: "mac" | "pc", m32: "mac" | "pc", extra: Partial<DeskState> = {}): DeskState {
  return { owner: { m34, m32 }, on: { mac: true, pc: m34 === "pc" || m32 === "pc" }, builtin_pref: "off", doubt: {}, ...extra };
}

/** Lo que anuncia cada máquina del puesto real. */
function machineCaps(machine: "mac" | "pc", extra: Partial<MachineCaps> = {}): MachineCaps {
  const pc = machine === "pc";
  return {
    machine,
    name: pc ? "ESCRITORIO" : "MacBook-Pro",
    os: pc ? "windows" : "macos",
    monitors: ["m34", "m32"].map((id) => ({ monitor: id, pnp_id: null, name: id, switch: pc, ddc: pc, seen: true })),
    builtin: !pc,
    hub: null,
    input: { capture: true, inject: true, missing: [] },
    power: { wake_peer: true, wakeable: true, power_off: pc },
    services: [],
    ...extra,
  };
}

function caps(agent: "mac" | "pc", peerSeen = true): Caps {
  return { here: machineCaps(agent), peer: peerSeen ? machineCaps(agent === "mac" ? "pc" : "mac") : null };
}

describe("modos", () => {
  it("se deducen del reparto", () => {
    expect(modeOf(desk, state("mac", "mac"))).toBe("mac");
    expect(modeOf(desk, state("pc", "pc"))).toBe("pc");
    expect(modeOf(desk, state("pc", "mac"))).toBe("shared");
    expect(modeOf({ monitors: [] }, state("mac", "mac"))).toBeNull();
  });

  it("un monitor sin dueño guardado está en la Mac", () => {
    expect(modeOf(desk, { owner: {}, on: { mac: true, pc: false }, builtin_pref: "off", doubt: {} })).toBe("mac");
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
      { id: "m34", label: '34"', owner: "pc", port: "DP", wide: true, movable: true, doubt: null },
    ]);
    expect(monitorsOf(desk, s, "mac").map((m) => [m.id, m.port, m.wide])).toEqual([["m32", "HDMI", false]]);
    // Sin confirmar (T-085), la escena lo marca.
    expect(monitorsOf(desk, { ...s, doubt: { m34: "stale" } }, "pc")[0].doubt).toBe("stale");
  });

  it("un monitor cableado a una sola máquina no se puede mover", () => {
    const solo: Desk = { monitors: [{ id: "x", label: "TV", wiring: { mac: "hdmi" } }] };
    expect(monitorsOf(solo, { owner: { x: "mac" }, on: { mac: true, pc: false }, builtin_pref: "off", doubt: {} }, "mac")[0].movable).toBe(false);
  });

  it("sin nadie que pueda cambiarle la entrada, tampoco (T-084)", () => {
    const s = state("pc", "mac");
    const nobody: Caps = { here: machineCaps("mac"), peer: machineCaps("pc", { monitors: [] }) };
    expect(canSwitch(nobody, "m34")).toBe(false);
    expect(monitorsOf(desk, s, "pc", nobody)[0].movable).toBe(false);
    // Lo pide a quien pueda, sea cual sea la ventana.
    expect(canSwitch(caps("mac"), "m34")).toBe(true);
    expect(canSwitch(caps("pc"), "m34")).toBe(true);
    // Sin anuncio de la otra se supone que puede lo que esta no, como el agente.
    expect(canSwitch(caps("mac", false), "m34")).toBe(true);
    expect(monitorsOf(desk, s, "pc", caps("mac", false))[0].movable).toBe(true);
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
    expect(builtinOf(desk, state("pc", "pc", { builtin_pref: "off", doubt: {} }))).toEqual({
      effective: "extend",
      forced: true,
      note: "encendida: no hay monitores",
    });
  });
});

const toPc: Step[] = [
  { step: "power_on", machine: "pc" },
  { step: "switch_input", monitor: "m34", from: "mac", to: "pc", input: "displayport", by: "pc" },
  { step: "attach", machine: "pc", monitor: "m34" },
  { step: "detach", machine: "mac", monitor: "m34" },
  { step: "switch_input", monitor: "m32", from: "mac", to: "pc", input: "displayport", by: "pc" },
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
        { step: "switch_input", monitor: "otro", from: "pc", to: "mac", input: "hdmi", by: "pc" },
        { step: "power_off", machine: "pc" },
        { step: "power_on", machine: "mac" },
      ]).map((g) => g.label),
    ).toEqual(["otro → HDMI", "Apagado limpio de la PC", "Encender la Mac"]);
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
    expect(peerPower(state("mac", "mac"), "mac", p)).toBe("boot");
    // Desde la PC, lo que arranca es la PC misma: no es la otra.
    expect(peerPower(state("mac", "mac"), "pc", p)).toBe("on");

    const later = run([
      { type: "started", txn: 1, steps: toPc },
      { type: "step_started", txn: 1, index: 0 },
      { type: "step_done", txn: 1, index: 0 },
      { type: "step_started", txn: 1, index: 1 },
      { type: "step_done", txn: 1, index: 1 },
    ])!;
    const groups = groupSteps(desk, toPc);
    expect(groups.map((g) => groupStatus(later, g))).toEqual(["done", "run", "pending", "pending", "pending"]);
    expect(peerPower(state("mac", "mac", { on: { mac: true, pc: true } }), "mac", later)).toBe("on");

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
    expect(peerPower(state("mac", "mac"), "mac", p)).toBe("off");
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

describe("par", () => {
  it("sin par emparejado lo dice", () => {
    expect(peerPill({ kind: "unpaired" })).toEqual({
      text: "sin par",
      title: "Sin par emparejado: esta máquina trabaja sola",
      ok: false,
    });
  });

  it("emparejado sin enlace avisa", () => {
    const p = peerPill({ kind: "paired", name: "PC", link: null });
    expect(p.text).toBe("sin enlace");
    expect(p.ok).toBe(false);
  });

  it("muestra el enlace activo; por Wi-Fi, que los servicios requieren cable", () => {
    expect(peerPill({ kind: "paired", name: "PC", link: "usb4" })).toEqual({ text: "USB4", title: "PC por USB4", ok: true });
    const wifi = peerPill({ kind: "paired", name: "PC", link: "wifi" });
    expect(wifi.text).toBe("Wi-Fi");
    expect(wifi.title).toContain("requieren cable");
  });
});

describe("chip de teclado y mouse", () => {
  const input = (extra: Partial<InputView> = {}): InputView => ({
    server: "pc",
    hub_here: false,
    session: true,
    locked: false,
    missing: [],
    problem: null,
    ...extra,
  });

  it("dónde está el teclado y quién lo recibe, con la sesión establecida", () => {
    expect(roleChip(input(), "mac", "pc", true)).toMatchObject({ kind: "keyboard", icon: "keyboard", text: "Tiene el teclado" });
    expect(roleChip(input(), "mac", "mac", true)).toMatchObject({ kind: "remote", icon: "cursor", text: "Teclado de la PC" });
    // Desde la PC, igual pero al revés: aquí es «Teclado aquí».
    expect(roleChip(input(), "pc", "pc", true)).toMatchObject({ kind: "keyboard", text: "Teclado aquí" });
    expect(roleChip(input({ server: "mac" }), "pc", "pc", true)).toMatchObject({ kind: "remote", text: "Teclado de la Mac" });
  });

  it("ningún texto habla de servidor ni de cliente (T-084)", () => {
    const texts: string[] = [];
    for (const server of ["mac", "pc"] as const) {
      for (const agent of ["mac", "pc"] as const) {
        for (const who of ["mac", "pc"] as const) {
          for (const peerOn of [true, false]) {
            for (const extra of [{}, { locked: true }, { session: false }]) {
              const c = roleChip(input({ server, ...extra }), agent, who, peerOn);
              if (c) texts.push(c.text, c.title);
            }
          }
        }
      }
    }
    expect(texts.length).toBeGreaterThan(0);
    for (const t of texts) expect(t.toLowerCase()).not.toMatch(/servidor|cliente/);
  });

  it("sin sesión el chip queda apagado y lo dice", () => {
    const c = roleChip(input({ session: false }), "mac", "pc", true);
    expect(c?.kind).toBe("idle");
    expect(c?.title).toContain("sin sesión");
  });

  it("el cursor bloqueado se ve en la que tiene el teclado", () => {
    const c = roleChip(input({ locked: true }), "pc", "pc", true);
    expect(c?.text).toBe("Teclado aquí · bloqueado");
    expect(c?.title).toContain("bloqueado");
  });

  it("con la otra apagada solo esta, si tiene el teclado", () => {
    expect(roleChip(input({ server: "mac" }), "mac", "mac", false)?.text).toBe("Teclado aquí");
    expect(roleChip(input({ server: "mac" }), "mac", "pc", false)).toBeNull();
    expect(roleChip(input(), "mac", "mac", false)).toBeNull();
    // Y en la PC con la Mac dormida, lo mismo.
    expect(roleChip(input(), "pc", "pc", false)?.text).toBe("Teclado aquí");
    expect(roleChip(input(), "pc", "mac", false)).toBeNull();
  });

  it("los permisos que faltan en esta máquina son un aviso", () => {
    const c = roleChip(input({ missing: ["accessibility", "input_monitoring"] }), "mac", "mac", true);
    expect(c).toMatchObject({ kind: "warn", icon: "warn", text: "faltan permisos" });
    expect(c?.title).toContain("Accesibilidad y Monitorización de entrada");
    // En la otra máquina no: son de esta.
    expect(roleChip(input({ missing: ["accessibility"] }), "mac", "pc", true)?.kind).toBe("keyboard");
    // Aunque la PC esté apagada.
    expect(roleChip(input({ missing: ["accessibility"] }), "mac", "mac", false)?.kind).toBe("warn");
  });

  it("un problema del motor de esta máquina también", () => {
    const c = roleChip(input({ problem: "no se pudo crear el CGEventTap" }), "mac", "mac", true);
    expect(c).toMatchObject({ kind: "warn", text: "sin teclado y mouse" });
    expect(c?.title).toContain("CGEventTap");
  });
});

describe("sin centro fijo (T-084)", () => {
  it("la tarjeta de la izquierda es siempre la del agente", () => {
    expect(sides("mac")).toEqual({ here: "mac", peer: "pc" });
    expect(sides("pc")).toEqual({ here: "pc", peer: "mac" });
  });

  it("un agente nunca se ve apagado a sí mismo", () => {
    for (const agent of ["mac", "pc"] as const) {
      // Aunque el estado venga con las dos apagadas (uno guardado viejo).
      const s: DeskState = { owner: { m34: agent, m32: agent }, on: { mac: false, pc: false }, builtin_pref: "off", doubt: {} };
      expect(machineState(s, agent, agent, null)).toBe("esta máquina");
      expect(machineState(s, agent, agent === "mac" ? "pc" : "mac", null)).toBe("apagada");
    }
  });

  it("en la PC, la PC está encendida y la que duerme es la Mac", () => {
    const s: DeskState = { owner: { m34: "pc", m32: "pc" }, on: { mac: false, pc: true }, builtin_pref: "off", doubt: {} };
    expect(machineState(s, "pc", "pc", null)).toBe("esta máquina");
    expect(machineState(s, "pc", "mac", null)).toBe("apagada");
    expect(peerPower(s, "pc", null)).toBe("off");
    expect(powerButton(caps("pc"), s, "pc", null)).toMatchObject({ visible: true, on: true, label: "Encender la Mac" });
  });

  it("el botón de encendido solo aparece si alguna puede", () => {
    const awake: DeskState = { owner: { m34: "pc", m32: "mac" }, on: { mac: true, pc: true }, builtin_pref: "off", doubt: {} };
    // La PC sabe apagarse limpio: desde la Mac se ofrece apagarla.
    expect(powerButton(caps("mac"), awake, "mac", null)).toMatchObject({ visible: true, on: false, label: "Apagar la PC" });
    // La Mac no: desde la PC no se ofrece apagarla.
    expect(powerButton(caps("pc"), awake, "pc", null).visible).toBe(false);
    // Una que dice que no se despierta por la red no ofrece encenderla.
    const asleep: DeskState = { ...awake, on: { mac: false, pc: true } };
    const noWake: Caps = { here: machineCaps("pc"), peer: machineCaps("mac", { power: { wake_peer: false, wakeable: false, power_off: false } }) };
    expect(powerButton(noWake, asleep, "pc", null).visible).toBe(false);
    // Sin su anuncio, se intenta (el agente explica si no se pudo).
    expect(powerButton(caps("pc", false), asleep, "pc", null).visible).toBe(true);
  });

  it("sin Wake-on-LAN en esta máquina no se ofrece encender la otra", () => {
    // Hallazgo del PR #56: la PC sin [power.wol] mostraba «Encender la Mac», que fallaba siempre.
    const asleep: DeskState = { owner: { m34: "pc", m32: "pc" }, on: { mac: false, pc: true }, builtin_pref: "off", doubt: {} };
    const noWol = machineCaps("pc", { power: { wake_peer: false, wakeable: true, power_off: true } });
    expect(powerButton({ here: noWol, peer: machineCaps("mac") }, asleep, "pc", null).visible).toBe(false);
    // Tampoco sin el anuncio de la Mac: lo que falta es de esta.
    expect(powerButton({ here: noWol, peer: null }, asleep, "pc", null).visible).toBe(false);
  });

  it("la pantalla integrada es de la que la anuncia", () => {
    expect(hasBuiltin(caps("pc"), "mac")).toBe(true);
    expect(hasBuiltin(caps("pc"), "pc")).toBe(false);
    expect(hasBuiltin(caps("mac"), "mac")).toBe(true);
    // Sin anuncio de la otra: la MacBook.
    expect(hasBuiltin(caps("pc", false), "mac")).toBe(true);
  });
});
