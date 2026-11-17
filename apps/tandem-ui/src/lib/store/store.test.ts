import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../bridge";
import type { Snapshot } from "../model";
import { formatHash, parseHash, SCREEN_TOPICS } from "../routes";
import { DemoSource, demoDesk, demoOptions, isDemo } from "./demo";
import { deskFromSnapshot, LiveSource } from "./live";
import { Store } from "./store.svelte";

afterEach(() => {
  vi.useRealTimers();
});

describe("rutas", () => {
  it("cada pantalla tiene su dirección y vuelve igual", () => {
    for (const hash of ["#/puesto", "#/tuneles", "#/monitores", "#/acomodo", "#/red", "#/ajustes", "#/maquina/a", "#/maquina/b"]) {
      expect(formatHash(parseHash(hash))).toBe(hash);
    }
    expect(parseHash("")).toEqual({ screen: "puesto" });
    expect(parseHash("#/maquina/c")).toEqual({ screen: "puesto" });
    expect(parseHash("#/nada")).toEqual({ screen: "puesto" });
  });
});

describe("store", () => {
  it("se suscribe solo a lo que muestra la pantalla visible", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    expect(src.watched.at(-1)).toEqual(SCREEN_TOPICS.puesto);
    store.go({ screen: "tuneles" });
    expect(src.watched.at(-1)).toEqual(["machines", "tunnels"]);
    // Otra máquina en la misma pantalla no vuelve a suscribir.
    store.go({ screen: "maquina", machine: "a" });
    const n = src.watched.length;
    store.go({ screen: "maquina", machine: "b" });
    expect(src.watched.length).toBe(n);
    expect(store.route).toEqual({ screen: "maquina", machine: "b" });
  });

  it("los recursos solo se mueven si la pantalla los muestra", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    const store = new Store(src);
    await store.start();
    const before = store.desk.machines.a?.resources?.history.join(",");
    vi.advanceTimersByTime(1600);
    expect(store.desk.machines.a?.resources?.history.join(",")).not.toBe(before);
    store.go({ screen: "red" });
    const frozen = store.desk.machines.a?.resources?.history.join(",");
    vi.advanceTimersByTime(5000);
    expect(store.desk.machines.a?.resources?.history.join(",")).toBe(frozen);
    store.stop();
  });

  it("cambia la dirección y la sigue", async () => {
    const loc = { hash: "#/red" };
    const store = new Store(new DemoSource(demoOptions("demo&still")), loc);
    expect(store.route).toEqual({ screen: "red" });
    store.go({ screen: "maquina", machine: "b" });
    expect(loc.hash).toBe("#/maquina/b");
    loc.hash = "#/acomodo";
    store.syncFromLocation();
    expect(store.route).toEqual({ screen: "acomodo" });
  });

  it("las acciones avisan lo que pasó", async () => {
    vi.useFakeTimers();
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    await store.applyScene("juego");
    expect(store.desk.activeScene).toBe("juego");
    expect(store.toasts.at(-1)).toMatchObject({ text: "Escena «Juego» aplicada", tone: "ok" });
    await store.applyScene("nada");
    expect(store.toasts.at(-1)).toMatchObject({ tone: "bad" });
    vi.advanceTimersByTime(5000);
    expect(store.toasts).toEqual([]);
  });

  it("encender, apagar, medir y módulos en el demo", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    await store.setPower("b", false);
    expect(store.desk.machines.b?.power).toBe("off");
    expect(store.desk.machines.b?.resources).toBeNull();
    await store.setPower("b", true);
    expect(store.desk.machines.b?.power).toBe("on");
    expect((await store.setPower("a", false)).ok).toBe(false);
    expect((await store.measureLink()).ok).toBe(true);
    await store.setModule("b", "clipboard", false);
    expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("off");
  });

  it("el contador de túneles cuenta los activos", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    expect(store.activeTunnels).toBe(3);
  });

  it("la paleta y el asistente se abren y se cierran", () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    store.togglePalette();
    expect(store.paletteOpen).toBe(true);
    store.openWizard();
    expect(store.paletteOpen).toBe(false);
    expect(store.wizardOpen).toBe(true);
    store.togglePalette(true);
    store.go({ screen: "red" });
    expect(store.paletteOpen).toBe(false);
  });

  it("si la fuente falla, el error se muestra", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    src.applyScene = () => Promise.reject(new Error("se cayó"));
    const store = new Store(src);
    const r = await store.applyScene("x");
    expect(r).toEqual({ ok: false, reason: "se cayó" });
  });
});

describe("modo demo", () => {
  it("se pide con ?demo", () => {
    expect(isDemo("?demo")).toBe(true);
    expect(isDemo("?demo=win-win&still")).toBe(true);
    expect(isDemo("?x=1")).toBe(false);
  });

  it("es determinista y coherente en cada combinación de sistemas (D2)", () => {
    for (const pair of ["mac-win", "win-win", "mac-mac"] as const) {
      const d = demoDesk({ pair, here: "a", trouble: "none", still: true });
      expect(d).toEqual(demoDesk({ pair, here: "a", trouble: "none", still: true }));
      expect(d.machines.a?.here).toBe(true);
      expect(d.machines.b?.here).toBe(false);
      expect(d.machines.a?.name).not.toBe(d.machines.b?.name);
      expect(d.machines.a?.color).not.toBe(d.machines.b?.color);
    }
    const ww = demoDesk(demoOptions("demo=win-win"));
    expect([ww.machines.a?.os, ww.machines.b?.os]).toEqual(["windows", "windows"]);
    const mm = demoDesk(demoOptions("demo=mac-mac&here=b"));
    expect([mm.machines.a?.os, mm.machines.b?.os]).toEqual(["macos", "macos"]);
    expect(mm.here).toBe("b");
  });

  it("cada estado difícil", () => {
    const off = demoDesk(demoOptions("demo&trouble=peer-off"));
    expect(off.trouble).toBe("peer-off");
    expect(off.troubleText).toBe("PC-Escritorio está apagada");
    expect(off.machines.b?.power).toBe("off");
    expect(off.link.kind).toBeNull();
    const down = demoDesk(demoOptions("demo&trouble=link-down"));
    expect(down.machines.b?.power).toBe("lost");
    const perms = demoDesk(demoOptions("demo&trouble=permissions"));
    expect(perms.machines.a?.modules.find((m) => m.id === "input")?.state).toBe("unavailable");
    expect(demoDesk(demoOptions("demo&trouble=no-agent")).troubleText).toContain("agente");
    expect(demoOptions("demo&trouble=otra").trouble).toBe("none");
  });
});

describe("fuente real", () => {
  const connected = (over: Partial<Extract<Snapshot, { kind: "connected" }>> = {}): Snapshot => ({
    kind: "connected",
    agent: "b",
    desk: { machines: { a: { os: "windows", name: "Taller", color: "#6cc59a" }, b: null }, screens: [], arrangement: { a_side: "left", offset: 0 } },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "Taller", link: "wifi" },
    pairing: { kind: "idle" },
    input: { server: "b", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
    caps: {
      here: {
        machine: "b",
        name: "Estudio",
        os: "windows",
        color: "#d9a75a",
        hub: null,
        input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: true },
        services: [],
      },
      peer: null,
    },
    ...over,
  });

  it("traduce la foto del agente: nombres, colores y enlace de cada lado", () => {
    const d = deskFromSnapshot(connected());
    expect(d.ready).toBe(true);
    expect(d.here).toBe("b");
    expect(d.machines.b).toMatchObject({ name: "Estudio", color: "#d9a75a", here: true, power: "on", detail: "Windows" });
    expect(d.machines.a).toMatchObject({ name: "Taller", color: "#6cc59a", here: false });
    expect(d.link.kind).toBe("wifi");
    expect(d.trouble).toBe("none");
  });

  it("los estados difíciles salen de la foto", () => {
    expect(deskFromSnapshot({ kind: "connecting" }).ready).toBe(false);
    expect(deskFromSnapshot({ kind: "unavailable", endpoint: null, reason: "no corre" })).toMatchObject({ trouble: "no-agent", troubleText: "no corre" });
    expect(deskFromSnapshot(connected({ state: { on: { a: false, b: true } } }))).toMatchObject({ trouble: "peer-off", troubleText: "Taller está apagada" });
    expect(deskFromSnapshot(connected({ peer: { kind: "paired", name: "Taller", link: null } })).trouble).toBe("link-down");
    const missing = connected();
    if (missing.kind === "connected") missing.input = { ...missing.input, missing: ["accessibility"] };
    expect(deskFromSnapshot(missing).trouble).toBe("permissions");
  });

  it("pide al agente lo que sabe hacer y dice lo que todavía no", async () => {
    const act = vi.fn(async () => ({ kind: "pong" as const, link: "wifi" as const, millis: 4 }));
    let push: ((s: Snapshot) => void) | undefined;
    const bridge = {
      snapshot: async () => connected(),
      onSnapshot: async (cb: (s: Snapshot) => void) => {
        push = cb;
      },
      act,
    } as unknown as Bridge;
    const src = new LiveSource(bridge);
    const store = new Store(src);
    await store.start();
    expect(store.desk.machines.b?.name).toBe("Estudio");
    push?.(connected({ caps: { ...((connected() as Extract<Snapshot, { kind: "connected" }>).caps), here: { ...(connected() as Extract<Snapshot, { kind: "connected" }>).caps.here, name: "Nuevo" } } }));
    expect(store.desk.machines.b?.name).toBe("Nuevo");
    expect(src.can("measure")).toBe(true);
    expect(src.can("scenes")).toBe(false);
    expect(await src.measureLink()).toEqual({ ok: true, text: "4 ms" });
    expect((await src.applyScene()).ok).toBe(false);
    expect((await src.setModule()).ok).toBe(false);
    act.mockResolvedValueOnce({ kind: "accepted", txn: 1 } as never);
    expect(await src.setPower("a", false)).toEqual({ ok: true });
    expect(act).toHaveBeenLastCalledWith({ kind: "set_power", machine: "a", on: false });
    src.watch(["link"]);
    expect(src.currentTopics).toEqual(["link"]);
  });
});
