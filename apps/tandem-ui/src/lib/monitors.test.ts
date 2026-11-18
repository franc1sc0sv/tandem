import { describe, expect, it } from "vitest";

import { EVENTS_SHOWN, accentOf, eventView, formatClock, monitorUi, monitorsUi, shortMonitor, whyNotSwitch } from "./monitors";
import type { MonitorJson } from "./model";

const std = (over: Partial<MonitorJson> = {}): MonitorJson => ({
  id: "g5-34",
  name: 'Odyssey G5 34"',
  compat: { Compatible: { id: "x" } } as unknown as MonitorJson["compat"],
  seen_by: ["a"],
  via: [
    ["a", "display_port"],
    ["b", "hdmi"],
  ],
  owner: "a",
  report: { id: "x", input: "hdmi", power_on: true, signal: true, events: [] },
  ...over,
});
const factory: MonitorJson = {
  id: "g5-32",
  name: 'Odyssey G5 32"',
  compat: { NotCompatible: { reason: "It joins only once its firmware meets the standard." } },
  seen_by: [],
  via: [],
  owner: null,
  report: null,
};

describe("monitores (V3-Monitors)", () => {
  it("el nombre corto es la diagonal en pulgadas", () => {
    expect(shortMonitor('Odyssey G5 34"')).toBe('34"');
    expect(shortMonitor("Sin medida")).toBe("Sin medida");
  });

  it("el acento del indicador es el del mockup para los colores por omisión y se oscurece para el resto", () => {
    expect(accentOf("#7aa7e6")).toBe("#4f7cbc");
    expect(accentOf("#A99BE8")).toBe("#6a5fa6");
    expect(accentOf("#ffffff")).toBe("#b3b3b3");
    expect(accentOf("rojo")).toBe("#3a3d45");
  });

  it("la hora del evento sale con dos cifras", () => {
    const d = new Date(2026, 0, 2, 3, 4).getTime();
    expect(formatClock(d)).toBe("03:04");
  });

  it("cada evento del firmware tiene su ícono y su texto", () => {
    expect(eventView({ at_ms: 1, kind: "SwitchRequested", input: "display_port" })).toMatchObject({ kind: "switch", text: "Input → DP, requested by Tandem" });
    expect(eventView({ at_ms: 1, kind: "Input", input: "hdmi" })).toMatchObject({ kind: "button", text: "Input → HDMI, with the buttons" });
    expect(eventView({ at_ms: 1, kind: "SwitchFailed", input: "hdmi" })).toMatchObject({ kind: "failed", text: "Switch to HDMI with no picture" });
    expect(eventView({ at_ms: 1, kind: "Boot", input: null })).toMatchObject({ kind: "boot" });
    expect(eventView({ at_ms: 1, kind: "Other", input: null })).toMatchObject({ kind: "other" });
  });

  it("un monitor compatible sabe qué máquina muestra por su entrada", () => {
    const m = monitorUi(std());
    expect(m).toMatchObject({ compat: { ok: true }, connected: true, owner: "a", input: "hdmi", shows: "b" });
    expect(m.ports).toEqual({ a: "display_port", b: "hdmi" });
  });

  it("D11: uno de fábrica es «no compatible», con su razón, y no tiene entrada", () => {
    expect(monitorUi(factory)).toMatchObject({ compat: { ok: false, reason: "It joins only once its firmware meets the standard." }, connected: false, shows: null });
  });

  it("los compatibles van primero y solo se listan los eventos más nuevos", () => {
    const events = Array.from({ length: 5 }, (_, i) => ({ at_ms: i, kind: "Signal" as const, input: null }));
    const list = monitorsUi([factory, std({ report: { id: "x", input: null, power_on: true, signal: true, events } })]);
    expect(list.map((m) => m.id)).toEqual(["g5-34", "g5-32"]);
    expect(list[0]?.events).toHaveLength(EVENTS_SHOWN);
    expect(list[0]?.events[0]?.at).toBe(4);
  });

  it("explica por qué no se puede pedir el cambio", () => {
    const ok = monitorUi(std());
    expect(whyNotSwitch(ok, "a", "PC")).toBeNull();
    expect(whyNotSwitch(monitorUi(factory), "a", "PC")).toMatch(/does not support/);
    expect(whyNotSwitch(monitorUi(std({ seen_by: [] })), "a", "PC")).toMatch(/is disconnected/);
    expect(whyNotSwitch(monitorUi(std({ via: [] })), "a", "PC")).toBe("It is not known which input PC uses");
  });
});
