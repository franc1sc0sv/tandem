import { describe, expect, it, vi } from "vitest";

import { buildCommands, filterCommands, flatten, move, normalize, shortName, type CommandContext } from "./commands";
import { demoDesk } from "./store/demo";

function ctx(can = true): CommandContext & { calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    go: (r) => calls.push(`go:${r.screen === "maquina" ? `maquina/${r.machine}` : r.screen}`),
    applyScene: async (id) => (calls.push(`scene:${id}`), { ok: true }),
    setPower: async (m, on) => (calls.push(`power:${m}:${on}`), { ok: true }),
    measureLink: async () => (calls.push("measure"), { ok: true }),
    lockHere: async (on) => (calls.push(`lock:${on}`), { ok: true }),
    openWizard: (step?: number) => calls.push(step ? `wizard:${step}` : "wizard"),
    can: () => can,
  };
}

const desk = demoDesk({ pair: "mac-win", here: "a", trouble: "none", still: true });

describe("comandos", () => {
  it("salen del puesto: escenas, the other machine, túneles abiertos, pantallas y acciones", () => {
    const groups = filterCommands(buildCommands(desk, ctx(), true), "");
    expect(groups.map((g) => g.name)).toEqual(["Scenes", "Machines", "Tunnels", "Go to", "Actions"]);
    const titles = flatten(groups).map((c) => c.title);
    expect(titles.slice(0, 4)).toEqual(["Apply “Work”", "Apply “MacBook only”", "Apply “Game”", "New scene…"]);
    expect(titles).toContain("Power off PC-Desktop");
    expect(titles).toContain("Change the other machine…");
    expect(titles).toContain("Measure the link");
    expect(titles).toContain("Open setup");
    // Los túneles cerrados no se ofrecen.
    expect(titles.some((t) => t.includes("ComfyUI"))).toBe(false);
  });

  it("ofrece «Lock here» o «Remove the lock» según el estado, y nada sin motor de entrada", () => {
    const titles = flatten(filterCommands(buildCommands(desk, ctx(), true), "lock")).map((c) => c.title);
    expect(titles).toEqual(["Lock here"]);
    const locked = flatten(filterCommands(buildCommands({ ...desk, locked: true }, ctx(), true), "lock")).map((c) => c.title);
    expect(locked).toEqual(["Remove the lock"]);
    expect(buildCommands(desk, ctx(false), true).some((c) => c.id.startsWith("lock:"))).toBe(false);
  });

  it("D1: nunca ofrece agregar una tercera máquina", () => {
    const titles = buildCommands(desk, ctx(), true).map((c) => c.title.toLowerCase());
    expect(titles.some((t) => t.includes("agregar"))).toBe(false);
  });

  it("la escena activa lo dice y lleva su atajo", () => {
    const trabajo = buildCommands(desk, ctx(), true).find((c) => c.id === "scene:trabajo");
    expect(trabajo).toMatchObject({ detail: "active", key: "⌃⌥1" });
  });

  it("lo que la fuente no sabe hacer no aparece", () => {
    const ids = buildCommands(desk, ctx(false), true).map((c) => c.id);
    expect(ids.some((i) => i.startsWith("scene:"))).toBe(false);
    expect(ids).not.toContain("link:measure");
    expect(ids.some((i) => i.startsWith("power:"))).toBe(false);
    expect(ids).toContain("go:red");
  });

  it("el atajo de Ajustes depende del sistema", () => {
    expect(buildCommands(desk, ctx(), true).find((c) => c.id === "go:ajustes")?.key).toBe("⌘,");
    expect(buildCommands(desk, ctx(), false).find((c) => c.id === "go:ajustes")?.key).toBe("Ctrl+,");
  });

  it("nombres y colores de cada máquina con dos Windows (D2)", () => {
    const ww = demoDesk({ pair: "win-win", here: "b", trouble: "none", still: true });
    const cmds = buildCommands(ww, ctx(), false);
    expect(cmds.find((c) => c.id === "power:a")?.title).toBe("Power off PC-Desktop");
    expect(cmds.find((c) => c.id === "go:maquina:b")).toMatchObject({ title: "PC-Workshop", color: "#6cc59a" });
  });
});

describe("túneles en la paleta", () => {
  it("usan los rótulos de la lista de Túneles: «Mac → PC» si los sistemas difieren", () => {
    const d = demoDesk({ pair: "mac-win", here: "a", trouble: "none", still: true, tunnels: "paleta", paleta: true });
    const t = buildCommands(d, ctx(), true).filter((c) => c.group === "Tunnels");
    expect(t.map((c) => [c.title, c.detail])).toEqual([
      ["Go to tunnel “ia-local”", "Mac → PC · active"],
      ["Go to tunnel “tandem”", "Mac → PC · active"],
    ]);
  });
});

describe("búsqueda", () => {
  const cmds = buildCommands(desk, ctx(), true);

  it("sin acentos ni mayúsculas, por palabras", () => {
    expect(normalize("  Tunnels ")).toBe("tunnels");
    const hits = flatten(filterCommands(cmds, "tunnels")).map((c) => c.id);
    expect(hits).toContain("go:tuneles");
    const both = flatten(filterCommands(cmds, "apply game")).map((c) => c.id);
    expect(both).toEqual(["scene:juego"]);
  });

  it("el nombre del grupo también cuenta", () => {
    const hits = flatten(filterCommands(cmds, "scenes")).map((c) => c.group);
    expect(new Set(hits)).toEqual(new Set(["Scenes"]));
  });

  it("sin resultados, nada", () => {
    expect(filterCommands(cmds, "zzz")).toEqual([]);
  });

  it("moverse da la vuelta", () => {
    expect(move(0, -1, 5)).toBe(4);
    expect(move(4, 1, 5)).toBe(0);
    expect(move(2, 1, 0)).toBe(0);
  });

  it("nombre corto de una máquina", () => {
    expect(shortName("MacBook Pro")).toBe("MacBook");
    expect(shortName("PC-Desktop")).toBe("PC-Desktop");
  });
});

describe("ejecutar", () => {
  it("cada comando pide lo suyo", async () => {
    const c = ctx();
    const cmds = buildCommands(desk, c, true);
    const cmdsLocked = buildCommands({ ...desk, locked: true }, c, true);
    const run = (id: string) => cmds.find((x) => x.id === id)?.run();
    await run("scene:juego");
    await run("power:b");
    await run("link:measure");
    await run("lock:on");
    await cmdsLocked.find((x) => x.id === "lock:off")?.run();
    run("wizard:open");
    run("go:maquina:a");
    run("pair:change");
    run("tunnel:t97");
    expect(c.calls).toEqual(["scene:juego", "power:b:false", "measure", "lock:true", "lock:false", "wizard", "go:maquina/a", "wizard:1", "go:tuneles"]);
  });

  it("con la otra apagada ofrece encenderla", () => {
    const off = demoDesk({ pair: "mac-win", here: "a", trouble: "peer-off", still: true });
    const spy = vi.fn(async () => ({ ok: true as const }));
    const cmds = buildCommands(off, { ...ctx(), setPower: spy }, true);
    const power = cmds.find((x) => x.id === "power:b");
    expect(power).toMatchObject({ title: "Power on PC-Desktop", detail: "off" });
    void power?.run();
    expect(spy).toHaveBeenCalledWith("b", true);
  });
});
