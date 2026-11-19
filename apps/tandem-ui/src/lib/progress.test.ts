import { describe, expect, it } from "vitest";

import type { Step, TxnView } from "./model";
import { BOOT_WAIT_MS, clockText, energyView, measureKind, measureView, powerTarget } from "./progress";
import { EMPTY_NETWORK, type NetworkView, type PathView } from "./store/types";
import { feedRun, type Run } from "./txn";

const steps: Step[] = [{ step: "power_on", machine: "b" }];
const name = (m: string) => (m === "a" ? "MacBook Pro" : "PC-Desktop");
function run(events: TxnView[], t0 = 0): Run {
  let r: Run | null = null;
  events.forEach((e, i) => {
    r = feedRun(r, e, t0 + i * 100);
  });
  if (!r) throw new Error("no run");
  return r;
}
const started: TxnView = { type: "started", txn: 2, steps };
const going: TxnView = { type: "step_started", txn: 2, index: 0 };

describe("power card (F-Transactions)", () => {
  it("waits with Wake-on-LAN, the clock against the maximum and what is left", () => {
    const v = energyView(run([started, going]), name, "MacBook Pro", 18_000);
    expect(v?.title).toBe("Powering on PC-Desktop");
    expect(v?.waiting).toBe(true);
    expect(v?.rows.map((r) => [r.icon, r.text, r.detail])).toEqual([
      ["done", "Wake-on-LAN sent from MacBook Pro", "every 5 s"],
      ["run", "Waiting for its agent to answer", "0:18 / 2:00"],
      ["todo", "Add it back to the desk", ""],
    ]);
    expect(v?.rows[2]?.dim).toBe(true);
  });

  it("ends well or fails with the agent's error", () => {
    const ok = energyView(
      run([started, going, { type: "step_done", txn: 2, index: 0 }, { type: "committed", txn: 2, state: { on: { a: true, b: true }, scene: null } }]),
      name,
      "MacBook Pro",
      99_000,
    );
    expect(ok?.waiting).toBe(false);
    expect(ok?.rows.map((r) => r.icon)).toEqual(["done", "done", "done"]);
    const bad = energyView(run([started, going, { type: "step_failed", txn: 2, index: 0, error: "did not answer en 2:00" }]), name, "MacBook Pro", 99_000);
    expect(bad?.rows[1]).toMatchObject({ icon: "bad", detail: "did not answer en 2:00", mono: false });
  });

  it("only draws power-on runs", () => {
    const scene = run([{ type: "started", txn: 1, steps: [{ step: "geometry" }] }]);
    expect(powerTarget(scene)).toBeNull();
    expect(energyView(scene, name, "x", 0)).toBeNull();
    expect(energyView(null, name, "x", 0)).toBeNull();
  });

  it("the clock has minutes and seconds and does not go below zero", () => {
    expect(clockText(0)).toBe("0:00");
    expect(clockText(BOOT_WAIT_MS)).toBe("2:00");
    expect(clockText(-5)).toBe("0:00");
    expect(clockText(65_900)).toBe("1:05");
  });
});

const path = (kind: PathView["kind"], state: PathView["state"], rttMs: number | null, gbps: number | null): PathView => ({
  kind,
  state,
  rttMs,
  inputP50Ms: null,
  inputP99Ms: null,
  targetMs: 4,
  gbps,
  history: [],
  detail: null,
});
const net = (measuring: NetworkView["measuring"] = "usb4"): NetworkView => ({
  ...EMPTY_NETWORK,
  paths: [path("usb4", "primary", 0.3, 9.4), path("ethernet", "unplugged", null, null), path("wifi", "backup", 3.8, 0.41)],
  measuring,
});

describe("measurement card (F-Transactions)", () => {
  it("draws what was measured, the bar and the other paths", () => {
    const v = measureView(net(), "usb4");
    expect(v).toMatchObject({ title: "Measuring the USB-C link", value: "9.4", unit: "Gbps", latency: "0.3 ms", percent: 94, aria: "9.4 of 10 Gbps", caption: "USB4 20 Gbps link" });
    expect(v?.others).toEqual([
      { name: "Ethernet", text: "no cable" },
      { name: "Wi-Fi", text: "410 Mbps · 3.8 ms" },
    ]);
  });

  it("dropped Wi-Fi says \"no signal\", not \"no cable\" (T-147)", () => {
    const n = net("usb4");
    n.paths = [path("usb4", "primary", 0.3, 9.4), path("ethernet", "unplugged", null, null), path("wifi", "unplugged", null, null)];
    expect(measureView(n, "usb4")?.others).toEqual([
      { name: "Ethernet", text: "no cable" },
      { name: "Wi-Fi", text: "no signal" },
    ]);
  });

  it("not measured yet shows dashes and the empty bar; Mbps under 1 Gbps", () => {
    const n = net();
    n.paths[0] = path("usb4", "primary", null, null);
    expect(measureView(n, "usb4")).toMatchObject({ value: "—", latency: "—", percent: 0 });
    expect(measureView(net("wifi"), "wifi")).toMatchObject({ value: "410", unit: "Mbps" });
  });

  it("the card is of the path being measured, or of the last one measured, or none", () => {
    expect(measureKind(net("wifi"), "usb4")).toBe("wifi");
    expect(measureKind(net(null), "usb4")).toBe("usb4");
    expect(measureKind(net(null), null)).toBeNull();
  });
});
