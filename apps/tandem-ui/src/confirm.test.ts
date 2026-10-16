import { afterEach, describe, expect, it, vi } from "vitest";

import { applyReady, complete, corrections, doubtNote, hintFor, initialAnswers, isConfirmed, questions, switchersOf } from "./confirm";
import type { Caps, Desk, DeskState, MachineCaps, Notice, PeerView, Snapshot } from "./model";
import { builtinControl, modeRequest } from "./scene";

const desk: Desk = {
  monitors: [
    { id: "m34", label: '34"', wiring: { pc: "displayport", mac: "hdmi" } },
    { id: "m32", label: '32"', wiring: { pc: "displayport", mac: "hdmi" } },
  ],
};

/** Sin estado guardado: todo «en la Mac», inventado (`State::initial`). */
function fresh(on: { mac: boolean; pc: boolean } = { mac: true, pc: false }): DeskState {
  return { owner: { m34: "mac", m32: "mac" }, on, builtin_pref: "off", doubt: { m34: "unknown", m32: "unknown" } };
}

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

const linked: PeerView = { kind: "paired", name: "PC", link: "usb4" };
const unlinked: PeerView = { kind: "paired", name: "PC", link: null };

describe("estado sin confirmar (T-085)", () => {
  it("sin estado guardado nada está confirmado; un monitor en negro sí cuenta", () => {
    expect(isConfirmed(desk, fresh())).toBe(false);
    const s: DeskState = { ...fresh(), doubt: { m32: "blank" } };
    expect(isConfirmed(desk, s)).toBe(true);
    expect(doubtNote(s, "m32")).toBe("en negro");
    expect(doubtNote(fresh(), "m34")).toBe("sin confirmar");
    expect(doubtNote(s, "m34")).toBeNull();
  });

  it("«Aplicar modo» con la PC encendida y enlazada; si no, se ofrece responder", () => {
    const on = fresh({ mac: true, pc: true });
    expect(applyReady(desk, on, caps("mac"), "mac", linked)).toEqual({ ready: true, why: null });
    const off = applyReady(desk, fresh(), caps("mac"), "mac", linked);
    expect(off.ready).toBe(false);
    expect(off.why).toContain("la PC");
    // Encendida pero sin enlace: no hay a quién pedirle el DDC.
    expect(applyReady(desk, on, caps("mac"), "mac", unlinked).ready).toBe(false);
    // Desde la PC, que cambia las entradas ella misma, siempre se puede.
    expect(applyReady(desk, fresh({ mac: false, pc: true }), caps("pc"), "pc", unlinked).ready).toBe(true);
  });

  it("sin anuncio de la otra se supone que ella cambia las entradas", () => {
    expect(switchersOf(caps("mac", false), "m34")).toEqual(["pc"]);
    expect(switchersOf(caps("pc", false), "m34")).toEqual(["pc"]);
  });

  it("pregunta por cada monitor sin confirmar, con pista si la hay", () => {
    const qs = questions(desk, fresh(), caps("mac"));
    expect(qs.map((q) => q.id)).toEqual(["m34", "m32"]);
    expect(qs[0].choices).toEqual(["mac", "pc"]);
    // La PC está apagada: lo único que se puede ver es la Mac.
    expect(qs[0].hint).toEqual({ shows: "mac", why: "la PC está apagada" });
    expect(initialAnswers(qs)).toEqual({ m34: "mac", m32: "mac" });
    expect(questions(desk, { ...fresh(), doubt: {} }, caps("mac"))).toEqual([]);
  });

  it("la pista sale de lo detectado o de lo último que se supo, y si no, ninguna", () => {
    const both = fresh({ mac: true, pc: true });
    const m34 = desk.monitors[0];
    expect(hintFor(m34, both, caps("mac"))).toBeNull();
    const onlyPc: Caps = { here: machineCaps("mac", { monitors: [{ monitor: "m34", pnp_id: null, name: "34", switch: false, ddc: false, seen: false }] }), peer: machineCaps("pc") };
    expect(hintFor(m34, both, onlyPc)).toEqual({ shows: "pc", why: "solo la PC lo detecta" });
    const stale: DeskState = { ...both, owner: { m34: "pc", m32: "mac" }, doubt: { m34: "stale", m32: "stale" } };
    expect(hintFor(m34, stale, caps("mac"))).toEqual({ shows: "pc", why: "lo último que se supo" });
    const single = { id: "x", label: "x", wiring: { mac: "hdmi" as const } };
    expect(hintFor(single, both, caps("mac"))?.shows).toBe("mac");
  });

  it("se manda solo con todas las respuestas, «nada» incluida", () => {
    const qs = questions(desk, fresh({ mac: true, pc: true }), caps("mac"));
    expect(complete(qs, {})).toBe(false);
    expect(complete(qs, { m34: "pc" })).toBe(false);
    expect(complete(qs, { m34: "pc", m32: null })).toBe(true);
  });

  it("corregir desde la escena ofrece las máquinas cableadas y «nada»", () => {
    const s: DeskState = { ...fresh(), owner: { m34: "pc", m32: "mac" }, doubt: {} };
    expect(corrections(desk.monitors[0], s)).toEqual([
      { shows: "mac", label: "la Mac", current: false },
      { shows: "pc", label: "la PC", current: true },
      { shows: null, label: "nada (negro)", current: false },
    ]);
    // Sin confirmar, ninguna opción es «la actual».
    expect(corrections(desk.monitors[0], fresh()).some((c) => c.current)).toBe(false);
  });

  it("la tarjeta de la PC nunca tiene el control de pantalla integrada", () => {
    for (const agent of ["mac", "pc"] as const) {
      expect(builtinControl(caps(agent), "pc")).toBe(false);
      expect(builtinControl(caps(agent), "mac")).toBe(true);
    }
    // Sin el anuncio de la Mac, tampoco se le dibuja el control.
    expect(builtinControl(caps("pc", false), "mac")).toBe(false);
  });
});

describe("modo demo: arranque sin estado y «Aplicar modo»", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  async function demo(query: string) {
    vi.useFakeTimers();
    // El demo atiende los atajos con `addEventListener` de la ventana.
    (globalThis as { addEventListener?: unknown }).addEventListener ??= () => {};
    const { demoBridge } = await import("./demo");
    const bridge = demoBridge(query);
    const notices: Notice[] = [];
    await bridge.onNotice(async (n) => {
      notices.push(n);
    });
    const connected = async () => {
      const s: Snapshot = await bridge.snapshot();
      if (s.kind !== "connected") throw new Error(s.kind);
      return s;
    };
    return { bridge, notices, connected };
  }

  it("ofrece «Aplicar modo» y responder; al aplicar queda confirmado con lo que se mandó", async () => {
    const { bridge, notices, connected } = await demo("?demo&fresh&pc=on");
    let s = await connected();
    expect(isConfirmed(s.desk, s.state)).toBe(false);
    expect(applyReady(s.desk, s.state, s.caps, s.agent, s.peer).ready).toBe(true);
    expect(questions(s.desk, s.state, s.caps)).toHaveLength(2);

    const r = await bridge.act({ kind: "apply_mode", mode: modeRequest("pc", s.desk, s.state) });
    expect(r.kind).toBe("accepted");
    await vi.runAllTimersAsync();
    const started = notices.find((n) => n.kind === "txn" && n.event.type === "started");
    if (!started || started.kind !== "txn" || started.event.type !== "started") throw new Error("sin transacción");
    const sent = started.event.steps.filter((st) => st.step === "switch_input");
    expect(sent.map((st) => st.step === "switch_input" && [st.monitor, st.to])).toEqual([
      ["m34", "pc"],
      ["m32", "pc"],
    ]);
    s = await connected();
    expect(isConfirmed(s.desk, s.state)).toBe(true);
    expect(s.state.doubt).toEqual({});
    expect(s.state.owner).toEqual({ m34: "pc", m32: "pc" });
  });

  it("responder lo que muestra confirma sin mandar entradas", async () => {
    const { bridge, notices, connected } = await demo("?demo&fresh");
    let s = await connected();
    expect(applyReady(s.desk, s.state, s.caps, s.agent, s.peer).ready).toBe(false);
    await bridge.act({ kind: "set_shown", shows: { m34: "mac", m32: null } });
    await vi.runAllTimersAsync();
    const steps = notices.flatMap((n) => (n.kind === "txn" && n.event.type === "started" ? n.event.steps : []));
    expect(steps.some((st) => st.step === "switch_input")).toBe(false);
    s = await connected();
    expect(isConfirmed(s.desk, s.state)).toBe(true);
    expect(s.state.doubt).toEqual({ m32: "blank" });
  });
});
