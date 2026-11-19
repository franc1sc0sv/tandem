import { describe, expect, it } from "vitest";

import { start } from "./crossing";
import type { Side } from "./model";
import { DemoSource, demoOptions } from "./store/demo";
import {
  NO_ACTIVITY,
  SKETCH,
  WIZARD_STEPS,
  arrowSide,
  clampStep,
  computeNote,
  crossVerdict,
  computeWorks,
  deviceRows,
  devicesOfHub,
  dropSide,
  foundRows,
  serviceRow,
  heard,
  missingOf,
  permCards,
  permFooter,
  stepMark,
  summaryRows,
  systemNote,
} from "./wizard";

async function desk(query = "demo&still") {
  let d = null as Awaited<ReturnType<typeof current>> | null;
  d = await current(query);
  return d;
}
async function current(query: string) {
  const s = new DemoSource(demoOptions(query));
  let out: Parameters<Parameters<DemoSource["start"]>[0]>[0] | null = null;
  await s.start((v) => (out = v));
  if (!out) throw new Error("no state");
  return out as Parameters<Parameters<DemoSource["start"]>[0]>[0];
}

describe("pasos", () => {
  it("son seis y cada uno es hecho, actual o por hacer", () => {
    expect(WIZARD_STEPS).toHaveLength(6);
    expect([0, 1, 2, 3, 4, 5].map((i) => stepMark(i, 2))).toEqual(["done", "done", "cur", "todo", "todo", "todo"]);
  });

  it("clampStep nunca sale de la lista", () => {
    for (let i = -50; i <= 50; i++) expect(clampStep(i)).toBeGreaterThanOrEqual(0), expect(clampStep(i)).toBeLessThanOrEqual(5);
  });
});

describe("paso 1", () => {
  it("la fila del sistema compone sistema, procesador y memoria", async () => {
    const d = await desk();
    const mac = d.machines.a!;
    expect(systemNote(mac)).toBe("macOS 26 · Apple M5 · 24 GB");
    expect(computeNote(mac)).toBe("zsh · Docker · uv · node");
    const rows = foundRows(mac);
    expect(rows.map((r) => r.label)).toEqual(["macOS 26 · Apple M5 · 24 GB", "Keyboard and mouse", "Clipboard", "Compute", "Monitors", "Administrator service"]);
    expect(rows.find((r) => r.id === "monitors")).toMatchObject({ state: "off", note: "no DDC channel right now; another machine controls them" });
    expect(rows.find((r) => r.id === "tunnels")?.mono).toBe(true);
  });

  it("la fila del servicio muestra los tres states que reporta la app y nunca gira sin dato", async () => {
    const d = await desk("demo&still&admin=pending");
    const rows = foundRows(d.machines.a!, { state: "pending", canInstall: true, note: "" });
    expect(rows.find((r) => r.id === "tunnels")).toMatchObject({ state: "ok", note: "zsh · Docker · uv · node" });
    expect(rows.at(-1)).toMatchObject({ id: "service", state: "wait", install: true });
    expect(serviceRow({ state: "off", canInstall: true, note: "" }, false)).toMatchObject({ state: "off", install: true });
    expect(serviceRow({ state: "ok", canInstall: false, note: "" }, false)).toMatchObject({ state: "ok", install: false });
    // Sin botón, la fila explica por qué (per-user install).
    expect(serviceRow({ state: "off", canInstall: false, note: "per-user install" }, false)).toMatchObject({ state: "off", note: "per-user install", install: false });
    // Sin lectura de la app: el módulo decide y, si no está activo, `off`; jamás `wait`.
    expect(serviceRow(null, true).state).toBe("ok");
    expect(serviceRow(null, false).state).toBe("off");
  });

  it("sin la machine no hay filas, y en Windows los monitors sí tienen canal", async () => {
    expect(foundRows(null)).toEqual([]);
    const d = await desk("demo&still");
    expect(foundRows(d.machines.b!).find((r) => r.id === "monitors")).toMatchObject({ state: "ok", note: "DDC channel" });
  });
});

describe("paso 3", () => {
  it("cuenta lo que falta y lo dice con los textos del mockup", async () => {
    const d = await desk("demo&still&perms=missing");
    const cards = permCards(d.permissions.rows);
    expect(cards.map((c) => [c.title, c.granted])).toEqual([
      ["Accessibility", false],
      ["Input Monitoring", false],
      ["Local Network", true],
    ]);
    expect(missingOf(cards)).toBe(2);
    // Lo que el agente no puede comprobar (Red local en un agente real) no bloquea ni se da por concedido.
    const unchecked = permCards([{ id: "accessibility", state: "granted" }, { id: "local_network", state: "unknown" }]);
    expect(unchecked.map((c) => [c.granted, c.missing])).toEqual([[true, false], [false, false]]);
    expect(missingOf(unchecked)).toBe(0);
    expect(permFooter(2)).toBe("Missing 2");
    expect(permFooter(1)).toBe("Missing 1");
    expect(permFooter(0)).toBe("All granted");
  });
});

describe("paso 4", () => {
  it("la actividad se acumula por machine y no cambia lo ya marcado", () => {
    const a = heard(NO_ACTIVITY, "b");
    expect(a).toEqual({ a: false, b: true });
    expect(heard(a, "b")).toBe(a);
    expect(heard(a, "a")).toEqual({ a: true, b: true });
  });

  it("lista solo lo que anuncia la machine: lo supuesto no cuenta", async () => {
    const d = await desk();
    expect(deviceRows(d.machines.a!).map((x) => x.name)).toEqual(["Built-in keyboard", "Trackpad", "Keychron K3 · MX Master 3S"]);
    expect(deviceRows(d.machines.b!)).toEqual([]);
    expect(deviceRows(null)).toEqual([]);
    // Con el módulo de entrada activo pero sin dispositivos anunciados, no hay fila.
    const { inputDevices: _omit, ...bare } = d.machines.a!;
    expect(deviceRows(bare)).toEqual([]);
  });

  it("el hub del agente es lo único que vale por teclado y mouse", () => {
    expect(devicesOfHub(true)).toEqual([{ name: "Keyboard and mouse", via: "through the hub" }]);
    expect(devicesOfHub(false)).toEqual([]);
    expect(devicesOfHub(null)).toEqual([]);
    expect(devicesOfHub(undefined)).toEqual([]);
  });
});

describe("paso 5", () => {
  it("cada borde tiene su dibujo", () => {
    for (const side of ["left", "right", "up", "down"] as Side[]) {
      expect(SKETCH[side].here).toContain("left");
      expect(SKETCH[side].edge).toContain("width");
    }
  });

  it("dropSide elige el lado dominante, y las flechas coinciden", () => {
    for (let dx = -40; dx <= 40; dx += 4) {
      for (let dy = -40; dy <= 40; dy += 4) {
        const side = dropSide(dx, dy);
        expect(side === "left" || side === "right" ? Math.abs(dx) >= Math.abs(dy) : Math.abs(dy) > Math.abs(dx)).toBe(true);
      }
    }
    expect([arrowSide("ArrowUp"), arrowSide("ArrowDown"), arrowSide("ArrowLeft"), arrowSide("ArrowRight"), arrowSide("Enter")]).toEqual(["up", "down", "left", "right", null]);
  });

  it("el verdict dice la verdad de cada fase", () => {
    const sample = { from: "a", to: "b", edge: "right", at: 1, of: 2, ms: 0.4 } as const;
    expect(crossVerdict(start(0), "right", "MacBook Pro", "PC", 20)).toMatchObject({ tone: "wait", text: "Waiting for the crossing at the right edge of MacBook Pro…" });
    expect(crossVerdict({ kind: "passed", sample }, "right", "M", "PC", 20)).toMatchObject({ tone: "ok", text: expect.stringContaining("Round trip measured: 0.4 ms") });
    expect(crossVerdict({ kind: "wrong-edge", sample: { ...sample, edge: "up" } }, "right", "M", "PC", 20).text).toContain("Crossed at the top, not the right");
    expect(crossVerdict({ kind: "timeout" }, "up", "M", "PC", 20).text).toContain("top edge");
    expect(crossVerdict({ kind: "unavailable", reason: "sin sonda" }, "up", "M", "PC", 20).tone).toBe("warn");
  });
});

describe("paso 6", () => {
  it("el resumen dice lo que se comprobó y lo que no", async () => {
    const d = await desk();
    const ok = summaryRows(d, { crossed: true });
    expect(ok.map((r) => [r.id, r.ok])).toEqual([
      ["link", true],
      ["input", true],
      ["monitors", true],
      ["tunnels", true],
    ]);
    expect(ok[0]?.note).toBe("USB-C · 0.3 ms");
    expect(ok[2]?.note).toContain("owner PC-Desktop");
    const no = summaryRows(d, { crossed: false });
    expect(no[1]).toMatchObject({ ok: false, note: "crossing not tested" });
  });

  it("el cómputo remoto exige el módulo encendido y un entorno listo", async () => {
    const d = await desk();
    type M = NonNullable<typeof d.machines.a>;
    const setTunnels = (m: M, state: "off" | "active" | "unavailable") => ({ ...m, modules: m.modules.map((x) => (x.id === "tunnels" ? { ...x, state } : x)) });
    const tunnels = (machines: typeof d.machines) => summaryRows({ ...d, machines }, { crossed: true }).find((r) => r.id === "tunnels");
    const a = d.machines.a!;
    const b = d.machines.b!;
    expect(a.environments.some((e) => e.ready)).toBe(true);
    expect(tunnels({ a, b })).toMatchObject({ ok: true, note: "MCP ready for Claude Code on both" });
    // Tunnels apagados en una machine, aunque haya entornos detectados.
    expect(tunnels({ a, b: setTunnels(b, "off") })?.ok).toBe(false);
    expect(tunnels({ a, b: setTunnels(b, "unavailable") })?.ok).toBe(false);
    // Entornos sin ninguno listo.
    const noReady = { ...b, environments: b.environments.map((e) => ({ ...e, ready: false })) };
    expect(tunnels({ a, b: noReady })?.ok).toBe(false);
    expect(computeWorks(null)).toBe(false);
  });
});
