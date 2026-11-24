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

describe("formats", () => {
  it("elapsed time reads in seconds, minutes and hours", () => {
    expect(elapsed(0)).toBe("0 s");
    expect(elapsed(45_000)).toBe("45 s");
    expect(elapsed(34 * 60_000)).toBe("34 m");
    expect(elapsed(60 * 60_000)).toBe("1 h");
    expect(elapsed((2 * 60 + 14) * 60_000)).toBe("2 h 14 m");
    // A clock set back does not give negative times.
    expect(elapsed(-5000)).toBe("0 s");
  });

  it("bytes use the unit that leaves the short number", () => {
    expect(bytesLabel(0)).toBe("0 B");
    expect(bytesLabel(999)).toBe("999 B");
    expect(bytesLabel(4e6)).toBe("4 MB");
    expect(bytesLabel(120e6)).toBe("120 MB");
    expect(bytesLabel(2.1e9)).toBe("2.1 GB");
    expect(bytesLabel(38e6)).toBe("38 MB");
  });

  it("the process panel labels", () => {
    expect(processesLabel(1)).toBe("1 process running");
    expect(processesLabel(12)).toBe("12 processes running");
    expect(latencyLabel(0.3)).toBe("0.3 ms");
    expect(latencyLabel(null)).toBe("—");
  });

  it("the short path names: by system if they differ, by name if not (D2)", () => {
    expect(shortLabels({ name: "MacBook Pro", os: "macos" }, { name: "PC-Desktop", os: "windows" })).toEqual(["Mac", "PC"]);
    expect(shortLabels({ name: "Office", os: "windows" }, { name: "Home-PC", os: "windows" })).toEqual(["Office", "Home"]);
  });

  it("with the same system and the same first word, what tells them apart (T-141)", () => {
    const w = (name: string) => ({ name, os: "windows" as const });
    const m = (name: string) => ({ name, os: "macos" as const });
    expect(shortLabels(w("PC-Desktop"), w("PC-Workshop"))).toEqual(["Desktop", "Workshop"]);
    expect(shortLabels(m("MacBook Pro"), m("Mac mini"))).toEqual(["Pro", "mini"]);
    expect(shortLabels(m("Mac Pro"), m("MacBook Pro"))).toEqual(["Mac Pro", "MacBook Pro"]);
  });

  it("two different names of the same system never give the same label", () => {
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

describe("state and projects with the demo (V3-Compute)", () => {
  const now = 1_763_400_000_000;
  const tunnels = demoTunnels(now);

  it("`tunnels=notes` (V3-Tray) moves the PR review to a third project", () => {
    expect(groupByProject(demoTunnels(now, "notes")).map((x) => x.name).sort()).toEqual(["ia-local", "notes", "tandem"]);
  });

  it("groups into two projects with their proportion of open", () => {
    const g = groupByProject(tunnels);
    expect(g.map((x) => [x.name, ratio(x)])).toEqual([
      ["tandem", "3/3"],
      ["ia-local", "1/2"],
    ]);
    expect(summary(g)).toBe("4 open · 2 projects");
  });

  it("without tunnels nothing is open, and with one it is said in the singular", () => {
    expect(summary([])).toBe("No open tunnels");
    expect(summary(groupByProject(tunnels.slice(0, 1)))).toBe("1 open · 1 project");
  });

  it("the state text: active, idle with its time, closed", () => {
    const [busy, , idle, , closed] = tunnels;
    expect(statusText(busy, now)).toBe("active");
    expect(statusText(idle, now)).toBe("inactive 9 m");
    expect(statusText({ ...idle, idleSince: null }, now)).toBe("inactive");
    expect(statusText(closed, now)).toBe("closed");
  });

  it("the empty panel says \"Tunnel closed\" or \"Nothing running right now\"", () => {
    expect(emptyLabel(tunnels[4])).toBe("Tunnel closed");
    expect(emptyLabel(tunnels[2])).toBe("Nothing running right now");
  });

  it("the tube runs faster the more active it is and turns off if closed", () => {
    const busy = tubeSpeed("busy");
    const idle = tubeSpeed("idle");
    expect(busy && idle && busy.a < idle.a).toBe(true);
    expect(tubeSpeed("closed")).toBeNull();
  });

  it("the requested one is chosen, or the first open, or the first", () => {
    expect(pickTunnel(tunnels, "lora")?.id).toBe("lora");
    expect(pickTunnel(tunnels, "missing")?.id).toBe("t97");
    expect(pickTunnel([tunnels[4]], null)?.id).toBe("comfy");
    expect(pickTunnel([], null)).toBeNull();
  });

  it("the demo processes come out in the order they were launched and one or the first is chosen", () => {
    const rows = demoProcesses("t97", now);
    expect(rows).toHaveLength(6);
    expect(runningOf([...rows].reverse()).map((r) => r.id)).toEqual([1, 2, 3, 4, 5, 6]);
    expect(pickProcess(rows, 3)?.id).toBe(3);
    expect(pickProcess(rows, 99)?.id).toBe(1);
    expect(pickProcess([], null)).toBeNull();
  });

  it("only those that are running count", () => {
    const rows = [processRow(proc(1)), processRow(proc(2, { kind: "exited", code: 0 })), processRow(proc(3, { kind: "killed" }))];
    expect(runningOf(rows).map((r) => r.id)).toEqual([1]);
    expect(rows[1]).toMatchObject({ state: "exited", code: 0 });
    expect(rows[2]).toMatchObject({ state: "killed", code: null });
  });
});

describe("from what the agent sends to what is drawn", () => {
  it("a tunnel with processes is active and without them, idle since it was opened", () => {
    const m = newMemory();
    const [busy, idle] = tunnelsFromWire([proj("tandem", [tun("1", [proc(1)]), tun("2")])], 5_000, m);
    expect(busy).toMatchObject({ state: "busy", processes: 1, project: "tandem", projectPath: "/src/tandem", from: "a", to: "b", branch: null, closedAt: null });
    expect(idle).toMatchObject({ state: "idle", processes: 0, idleSince: 1_000 });
  });

  it("a tunnel that becomes idle counts from when the window saw it that way", () => {
    const m = newMemory();
    tunnelsFromWire([proj("p", [tun("1", [proc(1)])])], 5_000, m);
    const [t] = tunnelsFromWire([proj("p", [tun("1")])], 9_000, m);
    expect(t).toMatchObject({ state: "idle", idleSince: 9_000 });
    // And if something runs again, it stops being idle.
    const [again] = tunnelsFromWire([proj("p", [tun("1", [proc(2)])])], 10_000, m);
    expect(again).toMatchObject({ state: "busy", idleSince: null });
  });

  it("tunnels without a project go under \"No project\"", () => {
    const [t] = tunnelsFromWire([proj(null, [tun("1")])], 0, newMemory());
    expect(t.project).toBe("No project");
    expect(t.projectPath).toBeNull();
  });

  it("what is no longer in the list stays as closed, at the end, with its time", () => {
    const m = newMemory();
    tunnelsFromWire([proj("p", [tun("1", [proc(1)]), tun("2")])], 5_000, m);
    const out = tunnelsFromWire([proj("p", [tun("2")])], 8_000, m);
    expect(out.map((t) => [t.id, t.state])).toEqual([
      ["2", "idle"],
      ["1", "closed"],
    ]);
    expect(out[1]).toMatchObject({ closedAt: 8_000, processes: 0 });
    // It stays closed in the following lists, with the same time.
    const next = tunnelsFromWire([proj("p", [tun("2")])], 9_000, m);
    expect(next[1]).toMatchObject({ state: "closed", closedAt: 8_000 });
  });

  it("does not keep more than a handful of closed ones", () => {
    const m = newMemory();
    const ids = Array.from({ length: CLOSED_KEPT + 5 }, (_, i) => String(i));
    tunnelsFromWire([proj("p", ids.map((id) => tun(id)))], 1, m);
    const out = tunnelsFromWire([], 2, m);
    expect(out).toHaveLength(CLOSED_KEPT);
    expect(out.every((t) => t.state === "closed")).toBe(true);
  });

  it("a tunnel the agent already marks closed is not drawn as open", () => {
    const out = tunnelsFromWire([proj("p", [tun("1", [], { open: false })])], 0, newMemory());
    expect(out).toEqual([]);
  });

  it("the traffic adds what each tunnel copied, without counting the same progress twice", () => {
    const m = newMemory();
    const t = (done: number, finished = false, upload = true) => ({ tunnel: "1", upload, path: "/a", done, total: 100, finished });
    addTransfer(m, t(30));
    addTransfer(m, t(30));
    addTransfer(m, t(100, true));
    addTransfer(m, t(40, false, false));
    expect(m.traffic.get("1")).toEqual({ up: 100, down: 40 });
    // A new copy of the same path counts from zero again.
    addTransfer(m, t(10));
    expect(m.traffic.get("1")?.up).toBe(110);
    const [view] = tunnelsFromWire([proj("p", [tun("1")])], 0, m);
    expect(view.traffic).toEqual({ up: 110, down: 40 });
  });
});

describe("live output", () => {
  const chunk = (text: string, stdout = text.length): OutputChunk => ({ text, next: { stdout, stderr: 0 }, done: false });

  it("joins the new and advances the cursor", () => {
    const a = appendOutput(EMPTY_OUTPUT, chunk("hello\n"));
    const b = appendOutput(a, chunk("world\n", 10));
    expect(b.text).toBe("hello\nworld\n");
    expect(b.cursor).toEqual({ stdout: 10, stderr: 0 });
  });

  it("withstands a lot of text by keeping only the end", () => {
    const big = "x".repeat(OUTPUT_KEEP) + "FIN";
    const o = appendOutput(EMPTY_OUTPUT, chunk(big));
    expect(o.text).toHaveLength(OUTPUT_KEEP);
    expect(o.text.endsWith("FIN")).toBe(true);
  });

  it("the agent's response brings stdout and stderr together and the cursors of each stream", () => {
    const c = outputChunk({ process: proc(1), stdout: "ok\n", stderr: "notice\n", next_stdout: 3, next_stderr: 6, done: true });
    expect(c).toEqual({ text: "ok\nnotice\n", next: { stdout: 3, stderr: 6 }, done: true });
  });
});

describe("machine row in the tray (T-167)", () => {
  const tun = (to: "a" | "b", state: "busy" | "idle" | "closed" = "busy") => ({ to, state }) as never;
  const mk = (id: "a" | "b", os: string, power = "on") => ({ id, os, detail: os === "macos" ? "macOS" : "Windows", power }) as never;

  it("singular and plural are correct", () => {
    expect(countTunnels(1)).toBe("1 tunnel");
    expect(countTunnels(2)).toBe("2 tunnels");
    expect(countTunnels(0)).toBe("0 tunnels");
  });

  it.each([
    ["mac-pc", "macos", "windows", "1 tunnel", "Windows"],
    ["mac-mac", "macos", "macos", "1 tunnel", "macOS"],
    ["pc-pc", "windows", "windows", "1 tunnel", "Windows"],
  ])("%s: a tunnel toward a is counted the same on any system", (_n, osA, osB, lineA, lineB) => {
    const tunnels = [tun("a"), tun("b", "idle"), tun("b", "closed")];
    expect(machineLine(mk("a", osA), tunnels)).toBe(lineA);
    expect(machineLine(mk("b", osB), tunnels)).toBe(lineB);
  });

  it("a tunnel toward a Mac is counted", () => {
    expect(machineLine(mk("b", "macos"), [tun("b")])).toBe("1 tunnel");
  });

  it("off takes precedence over the count", () => {
    expect(machineLine(mk("a", "macos", "off"), [tun("a")])).toBe("off");
  });
});
