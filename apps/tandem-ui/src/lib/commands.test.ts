import { describe, expect, it, vi } from "vitest";

import { buildCommands, filterCommands, flatten, move, normalize, shortName, type CommandContext } from "./commands";
import { demoDesk } from "./store/demo";

function ctx(can = true): CommandContext & { calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    go: (r) => calls.push(`go:${r.screen === "machine" ? `machine/${r.machine}` : r.screen}`),
    applyScene: async (id) => (calls.push(`scene:${id}`), { ok: true }),
    setPower: async (m, on) => (calls.push(`power:${m}:${on}`), { ok: true }),
    measureLink: async () => (calls.push("measure"), { ok: true }),
    lockHere: async (on) => (calls.push(`lock:${on}`), { ok: true }),
    openWizard: (step?: number) => calls.push(step ? `wizard:${step}` : "wizard"),
    can: () => can,
  };
}

const desk = demoDesk({ pair: "mac-win", here: "a", trouble: "none", still: true });

describe("commands", () => {
  it("come from the desk: scenes, the other machine, open tunnels, screens and actions", () => {
    const groups = filterCommands(buildCommands(desk, ctx(), true), "");
    expect(groups.map((g) => g.name)).toEqual(["Scenes", "Machines", "Tunnels", "Go to", "Actions"]);
    const titles = flatten(groups).map((c) => c.title);
    expect(titles.slice(0, 4)).toEqual(["Apply “Work”", "Apply “MacBook only”", "Apply “Game”", "New scene…"]);
    expect(titles).toContain("Power off PC-Desktop");
    expect(titles).toContain("Change the other machine…");
    expect(titles).toContain("Measure the link");
    expect(titles).toContain("Open setup");
    // Closed tunnels are not offered.
    expect(titles.some((t) => t.includes("ComfyUI"))).toBe(false);
  });

  it("offers \"Lock here\" or \"Remove the lock\" depending on the state, and nothing without an input engine", () => {
    const titles = flatten(filterCommands(buildCommands(desk, ctx(), true), "lock")).map((c) => c.title);
    expect(titles).toEqual(["Lock here"]);
    const locked = flatten(filterCommands(buildCommands({ ...desk, locked: true }, ctx(), true), "lock")).map((c) => c.title);
    expect(locked).toEqual(["Remove the lock"]);
    expect(buildCommands(desk, ctx(false), true).some((c) => c.id.startsWith("lock:"))).toBe(false);
  });

  it("D1: never offers adding a third machine", () => {
    const titles = buildCommands(desk, ctx(), true).map((c) => c.title.toLowerCase());
    expect(titles.some((t) => t.includes("add"))).toBe(false);
  });

  it("the active scene says so and carries its shortcut", () => {
    const work = buildCommands(desk, ctx(), true).find((c) => c.id === "scene:trabajo");
    expect(work).toMatchObject({ detail: "active", key: "⌃⌥1" });
  });

  it("what the source cannot do does not appear", () => {
    const ids = buildCommands(desk, ctx(false), true).map((c) => c.id);
    expect(ids.some((i) => i.startsWith("scene:"))).toBe(false);
    expect(ids).not.toContain("link:measure");
    expect(ids.some((i) => i.startsWith("power:"))).toBe(false);
    expect(ids).toContain("go:network");
  });

  it("the Settings shortcut depends on the system", () => {
    expect(buildCommands(desk, ctx(), true).find((c) => c.id === "go:settings")?.key).toBe("⌘,");
    expect(buildCommands(desk, ctx(), false).find((c) => c.id === "go:settings")?.key).toBe("Ctrl+,");
  });

  it("names and colors of each machine with two Windows (D2)", () => {
    const ww = demoDesk({ pair: "win-win", here: "b", trouble: "none", still: true });
    const cmds = buildCommands(ww, ctx(), false);
    expect(cmds.find((c) => c.id === "power:a")?.title).toBe("Power off PC-Desktop");
    expect(cmds.find((c) => c.id === "go:machine:b")).toMatchObject({ title: "PC-Workshop", color: "#6cc59a" });
  });
});

describe("tunnels en la palette", () => {
  it("use the labels of the Tunnels list: \"Mac → PC\" if the systems differ", () => {
    const d = demoDesk({ pair: "mac-win", here: "a", trouble: "none", still: true, tunnels: "palette", palette: true });
    const t = buildCommands(d, ctx(), true).filter((c) => c.group === "Tunnels");
    expect(t.map((c) => [c.title, c.detail])).toEqual([
      ["Go to tunnel “ia-local”", "Mac → PC · active"],
      ["Go to tunnel “tandem”", "Mac → PC · active"],
    ]);
  });
});

describe("search", () => {
  const cmds = buildCommands(desk, ctx(), true);

  it("without accents or case, by words", () => {
    expect(normalize("  Tunnels ")).toBe("tunnels");
    const hits = flatten(filterCommands(cmds, "tunnels")).map((c) => c.id);
    expect(hits).toContain("go:tunnels");
    const both = flatten(filterCommands(cmds, "apply game")).map((c) => c.id);
    expect(both).toEqual(["scene:game"]);
  });

  it("the group name also counts", () => {
    const hits = flatten(filterCommands(cmds, "scenes")).map((c) => c.group);
    expect(new Set(hits)).toEqual(new Set(["Scenes"]));
  });

  it("without results, nothing", () => {
    expect(filterCommands(cmds, "zzz")).toEqual([]);
  });

  it("moving wraps around", () => {
    expect(move(0, -1, 5)).toBe(4);
    expect(move(4, 1, 5)).toBe(0);
    expect(move(2, 1, 0)).toBe(0);
  });

  it("short name of a machine", () => {
    expect(shortName("MacBook Pro")).toBe("MacBook");
    expect(shortName("PC-Desktop")).toBe("PC-Desktop");
  });
});

describe("run", () => {
  it("each command asks for its own thing", async () => {
    const c = ctx();
    const cmds = buildCommands(desk, c, true);
    const cmdsLocked = buildCommands({ ...desk, locked: true }, c, true);
    const run = (id: string) => cmds.find((x) => x.id === id)?.run();
    await run("scene:game");
    await run("power:b");
    await run("link:measure");
    await run("lock:on");
    await cmdsLocked.find((x) => x.id === "lock:off")?.run();
    run("wizard:open");
    run("go:machine:a");
    run("pair:change");
    run("tunnel:t97");
    expect(c.calls).toEqual(["scene:game", "power:b:false", "measure", "lock:true", "lock:false", "wizard", "go:machine/a", "wizard:1", "go:tunnels"]);
  });

  it("with the other one off it offers to power it on", () => {
    const off = demoDesk({ pair: "mac-win", here: "a", trouble: "peer-off", still: true });
    const spy = vi.fn(async () => ({ ok: true as const }));
    const cmds = buildCommands(off, { ...ctx(), setPower: spy }, true);
    const power = cmds.find((x) => x.id === "power:b");
    expect(power).toMatchObject({ title: "Power on PC-Desktop", detail: "off" });
    void power?.run();
    expect(spy).toHaveBeenCalledWith("b", true);
  });
});
