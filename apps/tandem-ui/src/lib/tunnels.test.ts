import fc from "fast-check";
import { describe, expect, it } from "vitest";

import { demoProcesses, demoTunnels } from "./store/demo";
import type { OutputChunk } from "./store/types";
import {
  CLOSED_KEPT,
  OUTPUT_KEEP,
  countTunnels,
  machineLine,
  addTransfer,
  appendOutput,
  bytesLabel,
  elapsed,
  emptyLabel,
  groupByProject,
  latencyLabel,
  newMemory,
  outputChunk,
  pickProcess,
  pickTunnel,
  processRow,
  processesLabel,
  ratio,
  runningOf,
  shortLabels,
  statusText,
  summary,
  tubeSpeed,
  tunnelsFromWire,
  EMPTY_OUTPUT,
  type ProcessWire,
  type ProjectWire,
  type TunnelWire,
} from "./tunnels";

const proc = (id: number, state: ProcessWire["state"] = { kind: "running" }): ProcessWire => ({ id, command: `cmd ${id}`, pid: 100 + id, started: 1000 * id, duration: null, state });
const tun = (id: string, running: ProcessWire[] = [], over: Partial<TunnelWire> = {}): TunnelWire => ({ id, title: `Tunnel ${id}`, origin: "a", target: "b", open: true, created: 1_000, running, ...over });
const proj = (name: string | null, tunnels: TunnelWire[]): ProjectWire => ({ name, root: name ? `/src/${name}` : null, tunnels });

describe("formatos", () => {
  it("el tiempo transcurrido se lee en segundos, minutos y horas", () => {
    expect(elapsed(0)).toBe("0 s");
    expect(elapsed(45_000)).toBe("45 s");
    expect(elapsed(34 * 60_000)).toBe("34 m");
    expect(elapsed(60 * 60_000)).toBe("1 h");
    expect(elapsed((2 * 60 + 14) * 60_000)).toBe("2 h 14 m");
    // Un reloj atrasado no da tiempos negativos.
    expect(elapsed(-5000)).toBe("0 s");
  });

  it("los bytes usan la unidad que deja el número corto", () => {
    expect(bytesLabel(0)).toBe("0 B");
    expect(bytesLabel(999)).toBe("999 B");
    expect(bytesLabel(4e6)).toBe("4 MB");
    expect(bytesLabel(120e6)).toBe("120 MB");
    expect(bytesLabel(2.1e9)).toBe("2.1 GB");
    expect(bytesLabel(38e6)).toBe("38 MB");
  });

  it("las etiquetas del panel de procesos", () => {
    expect(processesLabel(1)).toBe("1 process running");
    expect(processesLabel(12)).toBe("12 processes running");
    expect(latencyLabel(0.3)).toBe("0.3 ms");
    expect(latencyLabel(null)).toBe("—");
  });

  it("los nombres cortos del camino: por sistema si difieren, por nombre si no (D2)", () => {
    expect(shortLabels({ name: "MacBook Pro", os: "macos" }, { name: "PC-Desktop", os: "windows" })).toEqual(["Mac", "PC"]);
    expect(shortLabels({ name: "Office", os: "windows" }, { name: "Home-PC", os: "windows" })).toEqual(["Office", "Home"]);
  });

  it("con el mismo sistema y la misma primera palabra, lo que distingue (T-141)", () => {
    const w = (name: string) => ({ name, os: "windows" as const });
    const m = (name: string) => ({ name, os: "macos" as const });
    expect(shortLabels(w("PC-Desktop"), w("PC-Workshop"))).toEqual(["Desktop", "Workshop"]);
    expect(shortLabels(m("MacBook Pro"), m("Mac mini"))).toEqual(["Pro", "mini"]);
    expect(shortLabels(m("Mac Pro"), m("MacBook Pro"))).toEqual(["Mac Pro", "MacBook Pro"]);
  });

  it("dos nombres distintos del mismo sistema nunca dan el mismo rótulo", () => {
    const name = fc.stringMatching(/^[A-Za-z]{1,5}([ -][A-Za-z]{1,5}){0,2}$/);
    fc.assert(
      fc.property(name, name, fc.constantFrom("macos", "windows"), (x, y, os) => {
        fc.pre(x !== y);
        const [la, lb] = shortLabels({ name: x, os }, { name: y, os });
        expect(la).not.toBe(lb);
      }),
    );
  });
});

describe("estado y proyectos con el demo (V3-Compute)", () => {
  const now = 1_763_400_000_000;
  const tunnels = demoTunnels(now);

  it("`tunnels=notas` (V3-Tray) saca la revisión del PR a un tercer proyecto", () => {
    expect(groupByProject(demoTunnels(now, "notes")).map((x) => x.name).sort()).toEqual(["ia-local", "notes", "tandem"]);
  });

  it("agrupa en dos proyectos con su proporción de abiertos", () => {
    const g = groupByProject(tunnels);
    expect(g.map((x) => [x.name, ratio(x)])).toEqual([
      ["tandem", "3/3"],
      ["ia-local", "1/2"],
    ]);
    expect(summary(g)).toBe("4 open · 2 projects");
  });

  it("sin tunnels no hay nada abierto, y con uno se dice en singular", () => {
    expect(summary([])).toBe("No open tunnels");
    expect(summary(groupByProject(tunnels.slice(0, 1)))).toBe("1 open · 1 project");
  });

  it("el texto de estado: activo, inactivo con su tiempo, closed", () => {
    const [busy, , idle, , closed] = tunnels;
    expect(statusText(busy, now)).toBe("active");
    expect(statusText(idle, now)).toBe("inactive 9 m");
    expect(statusText({ ...idle, idleSince: null }, now)).toBe("inactive");
    expect(statusText(closed, now)).toBe("closed");
  });

  it("el panel vacío dice «Tunnel closed» o «Nothing running right now»", () => {
    expect(emptyLabel(tunnels[4])).toBe("Tunnel closed");
    expect(emptyLabel(tunnels[2])).toBe("Nothing running right now");
  });

  it("el tubo corre más rápido mientras más activo y se apaga si está closed", () => {
    const busy = tubeSpeed("busy");
    const idle = tubeSpeed("idle");
    expect(busy && idle && busy.a < idle.a).toBe(true);
    expect(tubeSpeed("closed")).toBeNull();
  });

  it("se elige el pedido, o el primero abierto, o el primero", () => {
    expect(pickTunnel(tunnels, "lora")?.id).toBe("lora");
    expect(pickTunnel(tunnels, "missing")?.id).toBe("t97");
    expect(pickTunnel([tunnels[4]], null)?.id).toBe("comfy");
    expect(pickTunnel([], null)).toBeNull();
  });

  it("los procesos del demo salen en el orden en que se lanzaron y se elige uno o el primero", () => {
    const rows = demoProcesses("t97", now);
    expect(rows).toHaveLength(6);
    expect(runningOf([...rows].reverse()).map((r) => r.id)).toEqual([1, 2, 3, 4, 5, 6]);
    expect(pickProcess(rows, 3)?.id).toBe(3);
    expect(pickProcess(rows, 99)?.id).toBe(1);
    expect(pickProcess([], null)).toBeNull();
  });

  it("solo cuentan los que corren", () => {
    const rows = [processRow(proc(1)), processRow(proc(2, { kind: "exited", code: 0 })), processRow(proc(3, { kind: "killed" }))];
    expect(runningOf(rows).map((r) => r.id)).toEqual([1]);
    expect(rows[1]).toMatchObject({ state: "exited", code: 0 });
    expect(rows[2]).toMatchObject({ state: "killed", code: null });
  });
});

describe("de lo que manda el agente a lo que se dibuja", () => {
  it("un tunnel con procesos está activo y sin ellos, inactivo desde que se abrió", () => {
    const m = newMemory();
    const [busy, idle] = tunnelsFromWire([proj("tandem", [tun("1", [proc(1)]), tun("2")])], 5_000, m);
    expect(busy).toMatchObject({ state: "busy", processes: 1, project: "tandem", projectPath: "/src/tandem", from: "a", to: "b", branch: null, closedAt: null });
    expect(idle).toMatchObject({ state: "idle", processes: 0, idleSince: 1_000 });
  });

  it("un tunnel que pasa a inactivo cuenta desde que la ventana lo vio así", () => {
    const m = newMemory();
    tunnelsFromWire([proj("p", [tun("1", [proc(1)])])], 5_000, m);
    const [t] = tunnelsFromWire([proj("p", [tun("1")])], 9_000, m);
    expect(t).toMatchObject({ state: "idle", idleSince: 9_000 });
    // Y si vuelve a correr algo, deja de estar inactivo.
    const [again] = tunnelsFromWire([proj("p", [tun("1", [proc(2)])])], 10_000, m);
    expect(again).toMatchObject({ state: "busy", idleSince: null });
  });

  it("los tunnels sin proyecto van bajo «No project»", () => {
    const [t] = tunnelsFromWire([proj(null, [tun("1")])], 0, newMemory());
    expect(t.project).toBe("No project");
    expect(t.projectPath).toBeNull();
  });

  it("lo que ya no está en la lista se queda como closed, al final, con su hora", () => {
    const m = newMemory();
    tunnelsFromWire([proj("p", [tun("1", [proc(1)]), tun("2")])], 5_000, m);
    const out = tunnelsFromWire([proj("p", [tun("2")])], 8_000, m);
    expect(out.map((t) => [t.id, t.state])).toEqual([
      ["2", "idle"],
      ["1", "closed"],
    ]);
    expect(out[1]).toMatchObject({ closedAt: 8_000, processes: 0 });
    // Sigue closed en las listas siguientes, con la misma hora.
    const next = tunnelsFromWire([proj("p", [tun("2")])], 9_000, m);
    expect(next[1]).toMatchObject({ state: "closed", closedAt: 8_000 });
  });

  it("no guarda más de unos cuantos closeds", () => {
    const m = newMemory();
    const ids = Array.from({ length: CLOSED_KEPT + 5 }, (_, i) => String(i));
    tunnelsFromWire([proj("p", ids.map((id) => tun(id)))], 1, m);
    const out = tunnelsFromWire([], 2, m);
    expect(out).toHaveLength(CLOSED_KEPT);
    expect(out.every((t) => t.state === "closed")).toBe(true);
  });

  it("un tunnel que el agente ya marca closed no se dibuja como abierto", () => {
    const out = tunnelsFromWire([proj("p", [tun("1", [], { open: false })])], 0, newMemory());
    expect(out).toEqual([]);
  });

  it("el tráfico suma lo copiado por cada tunnel, sin contar dos veces el mismo avance", () => {
    const m = newMemory();
    const t = (done: number, finished = false, upload = true) => ({ tunnel: "1", upload, path: "/a", done, total: 100, finished });
    addTransfer(m, t(30));
    addTransfer(m, t(30));
    addTransfer(m, t(100, true));
    addTransfer(m, t(40, false, false));
    expect(m.traffic.get("1")).toEqual({ up: 100, down: 40 });
    // Una copia nueva de la misma ruta vuelve a contar desde cero.
    addTransfer(m, t(10));
    expect(m.traffic.get("1")?.up).toBe(110);
    const [view] = tunnelsFromWire([proj("p", [tun("1")])], 0, m);
    expect(view.traffic).toEqual({ up: 110, down: 40 });
  });
});

describe("la salida en vivo", () => {
  const chunk = (text: string, stdout = text.length): OutputChunk => ({ text, next: { stdout, stderr: 0 }, done: false });

  it("junta lo fresh y avanza el cursor", () => {
    const a = appendOutput(EMPTY_OUTPUT, chunk("hello\n"));
    const b = appendOutput(a, chunk("world\n", 10));
    expect(b.text).toBe("hello\nworld\n");
    expect(b.cursor).toEqual({ stdout: 10, stderr: 0 });
  });

  it("aguanta mucho texto guardando solo el final", () => {
    const big = "x".repeat(OUTPUT_KEEP) + "FIN";
    const o = appendOutput(EMPTY_OUTPUT, chunk(big));
    expect(o.text).toHaveLength(OUTPUT_KEEP);
    expect(o.text.endsWith("FIN")).toBe(true);
  });

  it("la respuesta del agente trae stdout y stderr juntos y los cursores de cada flujo", () => {
    const c = outputChunk({ process: proc(1), stdout: "ok\n", stderr: "notice\n", next_stdout: 3, next_stderr: 6, done: true });
    expect(c).toEqual({ text: "ok\nnotice\n", next: { stdout: 3, stderr: 6 }, done: true });
  });
});

describe("renglón de machine en la tray (T-167)", () => {
  const tun = (to: "a" | "b", state: "busy" | "idle" | "closed" = "busy") => ({ to, state }) as never;
  const mk = (id: "a" | "b", os: string, power = "on") => ({ id, os, detail: os === "macos" ? "macOS" : "Windows", power }) as never;

  it("singular y plural correctos", () => {
    expect(countTunnels(1)).toBe("1 tunnel");
    expect(countTunnels(2)).toBe("2 tunnels");
    expect(countTunnels(0)).toBe("0 tunnels");
  });

  it.each([
    ["mac-pc", "macos", "windows", "1 tunnel", "Windows"],
    ["mac-mac", "macos", "macos", "1 tunnel", "macOS"],
    ["pc-pc", "windows", "windows", "1 tunnel", "Windows"],
  ])("%s: un tunnel hacia a se cuenta igual en cualquier sistema", (_n, osA, osB, lineA, lineB) => {
    const tunnels = [tun("a"), tun("b", "idle"), tun("b", "closed")];
    expect(machineLine(mk("a", osA), tunnels)).toBe(lineA);
    expect(machineLine(mk("b", osB), tunnels)).toBe(lineB);
  });

  it("un tunnel hacia una Mac se cuenta", () => {
    expect(machineLine(mk("b", "macos"), [tun("b")])).toBe("1 tunnel");
  });

  it("apagada manda sobre el conteo", () => {
    expect(machineLine(mk("a", "macos", "off"), [tun("a")])).toBe("off");
  });
});
