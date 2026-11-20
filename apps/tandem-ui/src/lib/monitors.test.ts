import { describe, expect, it } from "vitest";

import { EVENTS_SHOWN, NO_CHANNEL, accentOf, eventView, formatClock, hiddenScreens, monitorUi, monitorsUi, shortMonitor, whyNotSwitch } from "./monitors";
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

describe("monitors (V3-Monitors)", () => {
  it("the short name is the diagonal in inches", () => {
    expect(shortMonitor('Odyssey G5 34"')).toBe('34"');
    expect(shortMonitor("Unmeasured")).toBe("Unmeasured");
  });

  it("the indicator accent is the mockup's for the default colors and darkens for the rest", () => {
    expect(accentOf("#7aa7e6")).toBe("#4f7cbc");
    expect(accentOf("#A99BE8")).toBe("#6a5fa6");
    expect(accentOf("#ffffff")).toBe("#b3b3b3");
    expect(accentOf("crimson")).toBe("#3a3d45");
  });

  it("the event time comes out with two digits", () => {
    const d = new Date(2026, 0, 2, 3, 4).getTime();
    expect(formatClock(d)).toBe("03:04");
  });

  it("each firmware event has its icon and its text", () => {
    expect(eventView({ at_ms: 1, kind: "SwitchRequested", input: "display_port" })).toMatchObject({ kind: "switch", text: "Input → DP, requested by Tandem" });
    expect(eventView({ at_ms: 1, kind: "Input", input: "hdmi" })).toMatchObject({ kind: "button", text: "Input → HDMI, with the buttons" });
    expect(eventView({ at_ms: 1, kind: "SwitchFailed", input: "hdmi" })).toMatchObject({ kind: "failed", text: "Switch to HDMI with no picture" });
    expect(eventView({ at_ms: 1, kind: "Boot", input: null })).toMatchObject({ kind: "boot" });
    expect(eventView({ at_ms: 1, kind: "Other", input: null })).toMatchObject({ kind: "other" });
  });

  it("T-340: one monitor carries both connections, also the one seen only by its EDID", () => {
    const m = monitorUi(std({ seen_by: ["a", "b"] }));
    expect(m.ports).toEqual({ a: "display_port", b: "hdmi" });
    const alone = monitorUi(std({ compat: "NoChannel", seen_by: ["b"], via: [["b", "hdmi"]], owner: null, report: null }));
    expect(alone).toMatchObject({ compat: { ok: false, reason: NO_CHANNEL }, connected: true, owner: null, ports: { b: "hdmi" } });
  });

  it("T-340: the screen a monitor is on the machine it does not show is hidden", () => {
    const shown = monitorsUi([std()]); // HDMI on screen: it shows b
    const cables = [{ monitor: "g5-34", a: { input: "display_port" as const, screen: "screen-1" }, b: { input: "hdmi" as const, screen: "screen-2" } }];
    expect(hiddenScreens(cables, shown)).toEqual({ a: ["screen-1"], b: [] });
    // The declared input rules over the EDID's.
    const swapped = [{ monitor: "g5-34", a: { input: "hdmi" as const, screen: "screen-1" }, b: { input: "display_port" as const, screen: "screen-2" } }];
    expect(hiddenScreens(swapped, shown)).toEqual({ a: [], b: ["screen-2"] });
    // Without a screen, without a reading or not compatible: nothing is hidden.
    expect(hiddenScreens([{ monitor: "g5-34", a: { input: "display_port" } }], shown)).toEqual({ a: [], b: [] });
    expect(hiddenScreens(cables, monitorsUi([std({ report: null })]))).toEqual({ a: [], b: [] });
    expect(hiddenScreens(cables, monitorsUi([{ ...factory, id: "g5-34" }]))).toEqual({ a: [], b: [] });
    expect(hiddenScreens(undefined, shown)).toEqual({ a: [], b: [] });
  });

  it("a compatible monitor knows which machine shows through its input", () => {
    const m = monitorUi(std());
    expect(m).toMatchObject({ compat: { ok: true }, connected: true, owner: "a", input: "hdmi", shows: "b" });
    expect(m.ports).toEqual({ a: "display_port", b: "hdmi" });
  });

  it("D11: a factory one is \"not compatible\", with its reason, and has no input", () => {
    expect(monitorUi(factory)).toMatchObject({ compat: { ok: false, reason: "It joins only once its firmware meets the standard." }, connected: false, shows: null });
  });

  it("the compatible ones go first and only the most recent events are listed", () => {
    const events = Array.from({ length: 5 }, (_, i) => ({ at_ms: i, kind: "Signal" as const, input: null }));
    const list = monitorsUi([factory, std({ report: { id: "x", input: null, power_on: true, signal: true, events } })]);
    expect(list.map((m) => m.id)).toEqual(["g5-34", "g5-32"]);
    expect(list[0]?.events).toHaveLength(EVENTS_SHOWN);
    expect(list[0]?.events[0]?.at).toBe(4);
  });

  it("explains why the change cannot be requested", () => {
    const ok = monitorUi(std());
    expect(whyNotSwitch(ok, "a", "PC")).toBeNull();
    expect(whyNotSwitch(monitorUi(factory), "a", "PC")).toMatch(/does not support/);
    expect(whyNotSwitch(monitorUi(std({ seen_by: [] })), "a", "PC")).toMatch(/is disconnected/);
    expect(whyNotSwitch(monitorUi(std({ via: [] })), "a", "PC")).toBe("It is not known which input PC uses");
  });
});
