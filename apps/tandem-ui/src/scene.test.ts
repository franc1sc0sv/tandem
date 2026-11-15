import { describe, expect, it } from "vitest";

import type { Caps, Desk, DeskState, InputView, MachineCaps, Step } from "./model";
import {
  MODES_SOON,
  MONITORS_SOON,
  SOON,
  advance,
  declaredScreens,
  isRunning,
  machineState,
  peerPill,
  peerPower,
  powerButton,
  roleChip,
  sides,
  stepLabel,
  type Progress,
} from "./scene";

const desk: Desk = { screens: [], arrangement: { mac_side: "left", offset: 0 } };

function on(mac: boolean, pc: boolean): DeskState {
  return { on: { mac, pc } };
}

/** Lo que anuncia cada máquina del puesto real. */
function machineCaps(machine: "mac" | "pc", extra: Partial<MachineCaps> = {}): MachineCaps {
  const pc = machine === "pc";
  return {
    machine,
    name: pc ? "ESCRITORIO" : "MacBook-Pro",
    os: pc ? "windows" : "macos",
    hub: null,
    input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 0, started: 0 } },
    power: { wake_peer: true, wakeable: true, power_off: pc },
    services: [],
    ...extra,
  };
}

function caps(agent: "mac" | "pc", peerSeen = true): Caps {
  return { here: machineCaps(agent), peer: peerSeen ? machineCaps(agent === "mac" ? "pc" : "mac") : null };
}

describe("monitores y modos en pausa (DESIGN §8)", () => {
  it("se dibujan deshabilitados con la leyenda del firmware propio", () => {
    expect(SOON).toBe("Próximamente: firmware propio");
    expect(MONITORS_SOON.map((m) => m.label)).toEqual(['34"', '32"']);
    expect(MONITORS_SOON[0]?.wide).toBe(true);
    expect([...MODES_SOON]).toEqual(["Solo Mac", "Compartido", "Solo Windows"]);
  });
});

describe("acomodo", () => {
  it("sin declarar, la Mac a la izquierda; el puesto puede ponerla a la derecha", () => {
    expect(sides("mac", desk)).toMatchObject({ left: "mac", right: "pc", here: "mac", peer: "pc" });
    expect(sides("pc")).toMatchObject({ left: "mac", right: "pc", here: "pc", peer: "mac" });
    const right: Desk = { ...desk, arrangement: { mac_side: "right", offset: 0 } };
    expect(sides("pc", right)).toMatchObject({ left: "pc", right: "mac" });
  });

  it("las pantallas declaradas son de cada máquina", () => {
    const d: Desk = {
      ...desk,
      screens: [
        { machine: "pc", id: "34", x: 0, y: 0, width: 3440, height: 1440, scale: 1 },
        { machine: "mac", id: "laptop", x: 0, y: 0, width: 1512, height: 982, scale: 2 },
      ],
    };
    expect(declaredScreens(d, "pc").map((s) => s.id)).toEqual(["34"]);
    expect(declaredScreens(desk, "mac")).toEqual([]);
  });
});

describe("progreso por pasos", () => {
  const steps: Step[] = [{ step: "power_on", machine: "pc" }];

  it("nombra cada paso", () => {
    expect(stepLabel({ step: "power_on", machine: "pc" })).toBe("Encender la PC");
    expect(stepLabel({ step: "power_off", machine: "mac" })).toBe("Apagar la Mac");
  });

  it("sigue cada paso hasta confirmar", () => {
    let p = advance(null, { type: "started", txn: 1, steps });
    expect(isRunning(p)).toBe(true);
    p = advance(p, { type: "step_started", txn: 1, index: 0 });
    expect(p?.status).toEqual(["run"]);
    p = advance(p, { type: "step_done", txn: 1, index: 0 });
    p = advance(p, { type: "committed", txn: 1, state: on(true, true) });
    expect(p?.outcome).toEqual({ kind: "committed" });
    expect(isRunning(p)).toBe(false);
  });

  it("un fallo se ve en su paso y en el desenlace", () => {
    let p = advance(null, { type: "started", txn: 2, steps });
    p = advance(p, { type: "step_failed", txn: 2, index: 0, error: "sin Wake-on-LAN" });
    expect(p?.status).toEqual(["failed"]);
    p = advance(p, { type: "failed", txn: 2, state: on(true, false) });
    expect(p?.outcome).toEqual({ kind: "failed", error: "sin Wake-on-LAN" });
  });

  it("ignora los avisos de otra transacción", () => {
    const p = advance(null, { type: "started", txn: 3, steps });
    expect(advance(p, { type: "step_done", txn: 9, index: 0 })).toBe(p);
    expect(advance(null, { type: "committed", txn: 3, state: on(true, true) })).toBeNull();
  });

  it("la otra máquina arranca mientras corre su encendido", () => {
    let p: Progress | null = advance(null, { type: "started", txn: 4, steps });
    p = advance(p, { type: "step_started", txn: 4, index: 0 });
    expect(peerPower(on(true, false), "mac", p)).toBe("boot");
    expect(machineState(on(true, false), "mac", "pc", p)).toBe("arrancando…");
    expect(powerButton(caps("mac"), on(true, false), "mac", p)).toMatchObject({ on: null, power: "boot" });
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
  it("sin definir no se supone la Mac: solo lo dice el chip de esta máquina (T-087)", () => {
    const undef = { server: "mac" as const, hub_here: null, session: true, locked: false, missing: [], problem: null, defined: false };
    expect(roleChip(undef, "mac", "mac", true)).toMatchObject({ kind: "idle", text: "Teclado sin definir" });
    expect(roleChip(undef, "mac", "pc", true)).toBeNull();
    expect(roleChip(undef, "pc", "pc", false)).toMatchObject({ text: "Teclado sin definir" });
    // Un permiso que falta se avisa antes.
    expect(roleChip({ ...undef, missing: ["accessibility"] }, "mac", "mac", true)).toMatchObject({ kind: "warn" });
  });

  const input = (extra: Partial<InputView> = {}): InputView => ({
    server: "pc",
    hub_here: false,
    session: true,
    locked: false,
    missing: [],
    problem: null,
    defined: true,
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
  it("un agente nunca se ve apagado a sí mismo", () => {
    for (const agent of ["mac", "pc"] as const) {
      // Aunque el estado venga con las dos apagadas (uno guardado viejo).
      const s = on(false, false);
      expect(machineState(s, agent, agent, null)).toBe("esta máquina");
      expect(machineState(s, agent, agent === "mac" ? "pc" : "mac", null)).toBe("apagada");
    }
  });

  it("en la PC, la PC está encendida y la que duerme es la Mac", () => {
    const s = on(false, true);
    expect(machineState(s, "pc", "pc", null)).toBe("esta máquina");
    expect(machineState(s, "pc", "mac", null)).toBe("apagada");
    expect(peerPower(s, "pc", null)).toBe("off");
    expect(powerButton(caps("pc"), s, "pc", null)).toMatchObject({ visible: true, on: true, label: "Encender la Mac" });
  });

  it("el botón de encendido solo aparece si alguna puede", () => {
    const awake = on(true, true);
    // La PC sabe apagarse limpio: desde la Mac se ofrece apagarla.
    expect(powerButton(caps("mac"), awake, "mac", null)).toMatchObject({ visible: true, on: false, label: "Apagar la PC" });
    // La Mac no: desde la PC no se ofrece apagarla.
    expect(powerButton(caps("pc"), awake, "pc", null).visible).toBe(false);
    // Una que dice que no se despierta por la red no ofrece encenderla.
    const asleep = on(false, true);
    const noWake: Caps = { here: machineCaps("pc"), peer: machineCaps("mac", { power: { wake_peer: false, wakeable: false, power_off: false } }) };
    expect(powerButton(noWake, asleep, "pc", null).visible).toBe(false);
    // Sin su anuncio, se intenta (el agente explica si no se pudo).
    expect(powerButton(caps("pc", false), asleep, "pc", null).visible).toBe(true);
  });

  it("sin Wake-on-LAN en esta máquina no se ofrece encender la otra", () => {
    // Hallazgo del PR #56: la PC sin [power.wol] mostraba «Encender la Mac», que fallaba siempre.
    const asleep = on(false, true);
    const noWol = machineCaps("pc", { power: { wake_peer: false, wakeable: true, power_off: true } });
    expect(powerButton({ here: noWol, peer: machineCaps("mac") }, asleep, "pc", null).visible).toBe(false);
    // Tampoco sin el anuncio de la Mac: lo que falta es de esta.
    expect(powerButton({ here: noWol, peer: null }, asleep, "pc", null).visible).toBe(false);
  });
});
