import { describe, expect, it } from "vitest";

import type { Step, TxnView } from "./model";
import { BOOT_WAIT_MS, clockText, energyView, measureKind, measureView, powerTarget } from "./progress";
import { EMPTY_NETWORK, type NetworkView, type PathView } from "./store/types";
import { feedRun, type Run } from "./txn";

const steps: Step[] = [{ step: "power_on", machine: "b" }];
const name = (m: string) => (m === "a" ? "MacBook Pro" : "PC-Escritorio");
function run(events: TxnView[], t0 = 0): Run {
  let r: Run | null = null;
  events.forEach((e, i) => {
    r = feedRun(r, e, t0 + i * 100);
  });
  if (!r) throw new Error("sin corrida");
  return r;
}
const started: TxnView = { type: "started", txn: 2, steps };
const going: TxnView = { type: "step_started", txn: 2, index: 0 };

describe("tarjeta de energía (F-Transacciones)", () => {
  it("espera con Wake-on-LAN, el reloj contra el máximo y lo que falta", () => {
    const v = energyView(run([started, going]), name, "MacBook Pro", 18_000);
    expect(v?.title).toBe("Encendiendo PC-Escritorio");
    expect(v?.waiting).toBe(true);
    expect(v?.rows.map((r) => [r.icon, r.text, r.detail])).toEqual([
      ["done", "Wake-on-LAN enviado desde MacBook Pro", "cada 5 s"],
      ["run", "Esperando a que su agente conteste", "0:18 / 2:00"],
      ["todo", "Volver a incluirla en el puesto", ""],
    ]);
    expect(v?.rows[2]?.dim).toBe(true);
  });

  it("termina bien o falla con el error del agente", () => {
    const ok = energyView(
      run([started, going, { type: "step_done", txn: 2, index: 0 }, { type: "committed", txn: 2, state: { on: { a: true, b: true }, scene: null } }]),
      name,
      "MacBook Pro",
      99_000,
    );
    expect(ok?.waiting).toBe(false);
    expect(ok?.rows.map((r) => r.icon)).toEqual(["done", "done", "done"]);
    const bad = energyView(run([started, going, { type: "step_failed", txn: 2, index: 0, error: "no contestó en 2:00" }]), name, "MacBook Pro", 99_000);
    expect(bad?.rows[1]).toMatchObject({ icon: "bad", detail: "no contestó en 2:00", mono: false });
  });

  it("solo dibuja corridas de encendido", () => {
    const scene = run([{ type: "started", txn: 1, steps: [{ step: "geometry" }] }]);
    expect(powerTarget(scene)).toBeNull();
    expect(energyView(scene, name, "x", 0)).toBeNull();
    expect(energyView(null, name, "x", 0)).toBeNull();
  });

  it("el reloj tiene minutos y segundos y no baja de cero", () => {
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

describe("tarjeta de medición (F-Transacciones)", () => {
  it("dibuja lo medido, la barra y los otros caminos", () => {
    const v = measureView(net(), "usb4");
    expect(v).toMatchObject({ title: "Midiendo el enlace USB-C", value: "9.4", unit: "Gbps", latency: "0.3 ms", percent: 94, aria: "9.4 de 10 Gbps", caption: "USB4 20 Gbps de enlace" });
    expect(v?.others).toEqual([
      { name: "Ethernet", text: "sin cable" },
      { name: "Wi-Fi", text: "410 Mbps · 3.8 ms" },
    ]);
  });

  it("el Wi-Fi caído dice «sin señal», no «sin cable» (T-147)", () => {
    const n = net("usb4");
    n.paths = [path("usb4", "primary", 0.3, 9.4), path("ethernet", "unplugged", null, null), path("wifi", "unplugged", null, null)];
    expect(measureView(n, "usb4")?.others).toEqual([
      { name: "Ethernet", text: "sin cable" },
      { name: "Wi-Fi", text: "sin señal" },
    ]);
  });

  it("sin medir todavía muestra guiones y la barra vacía; Mbps bajo 1 Gbps", () => {
    const n = net();
    n.paths[0] = path("usb4", "primary", null, null);
    expect(measureView(n, "usb4")).toMatchObject({ value: "—", latency: "—", percent: 0 });
    expect(measureView(net("wifi"), "wifi")).toMatchObject({ value: "410", unit: "Mbps" });
  });

  it("la tarjeta es del camino que se mide, o del último medido, o ninguna", () => {
    expect(measureKind(net("wifi"), "usb4")).toBe("wifi");
    expect(measureKind(net(null), "usb4")).toBe("usb4");
    expect(measureKind(net(null), null)).toBeNull();
  });
});
