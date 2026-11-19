import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../bridge";
import type { Notice, SceneJson, Snapshot, TxnView } from "../model";
import { exitSide } from "../acomodo";
import { formatHash, parseHash, SCREEN_TOPICS } from "../routes";
import { DEMO_NOW, DemoSource, demoDesk, demoOptions, isDemo } from "./demo";
import { deskFromSnapshot, layoutFromDesk, LiveSource } from "./live";
import { SHELL_TOPICS, Store, TOAST_MS, TRAY_TOPICS, topicsFor } from "./store.svelte";

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
  it("se suscribe a lo que el shell muestra siempre más lo de la pantalla visible", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    const set = (t: string[] | undefined) => new Set(t);
    expect(set(src.watched.at(-1))).toEqual(new Set([...SHELL_TOPICS, ...SCREEN_TOPICS.puesto]));
    // El contador de túneles de la barra lateral sigue vivo en cualquier pantalla.
    store.go({ screen: "red" });
    expect(set(src.watched.at(-1))).toEqual(new Set(["machines", "tunnels", "link", "network"]));
    store.go({ screen: "tuneles" });
    expect(set(src.watched.at(-1))).toEqual(new Set(["machines", "tunnels"]));
    for (const screen of ["puesto", "tuneles", "monitores", "acomodo", "red", "ajustes"] as const) {
      expect(topicsFor({ screen })).toEqual(expect.arrayContaining(SHELL_TOPICS));
      expect(new Set(topicsFor({ screen })).size).toBe(topicsFor({ screen }).length);
    }
    // Other machine en la misma pantalla no vuelve a suscribir.
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
    await store.applyScene("game");
    expect(store.desk.activeScene).toBe("game");
    expect(store.toasts[0]).toMatchObject({ text: "Scene “Game” active.", tone: "ok" });
    await store.applyScene("nada");
    expect(store.toasts[0]).toMatchObject({ tone: "bad" });
    vi.advanceTimersByTime(TOAST_MS);
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

describe("bandeja", () => {
  it("pide su propio conjunto de temas y no cambia con la ruta", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src, null, TRAY_TOPICS);
    await store.start();
    expect(src.watched).toEqual([TRAY_TOPICS]);
    store.go({ screen: "red" });
    expect(src.watched).toEqual([TRAY_TOPICS]);
    expect(TRAY_TOPICS).toEqual(expect.arrayContaining(["machines", "link", "scenes", "tunnels", "resources"]));
  });
});

describe("errores del agente", () => {
  it("una foto a la que le faltan campos da «sin agente» con la razón, no «conectando»", async () => {
    const broken = { kind: "connected", agent: "a", desk: { screens: [], arrangement: { a_side: "left", offset: 0 } } };
    const bridge = { snapshot: async () => broken, onSnapshot: async () => {} } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    expect(store.desk.ready).toBe(true);
    expect(store.desk.trouble).toBe("no-agent");
    expect(store.desk.troubleText).toContain("does not understand");
  });

  it("un agente de la versión anterior (lados mac/pc) también", () => {
    const old = { kind: "connected", agent: "mac", desk: { screens: [], arrangement: { mac_side: "left", offset: 0 } } };
    expect(() => deskFromSnapshot(old as unknown as Snapshot)).toThrow(/side “mac”/);
  });

  it("si la fuente no arranca, se muestra el error", async () => {
    const bridge = { snapshot: async () => Promise.reject(new Error("sin núcleo")), onSnapshot: async () => {} } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    expect(store.desk).toMatchObject({ ready: true, trouble: "no-agent", troubleText: "sin núcleo" });
  });

  it("una foto rota después de arrancar no tumba la ventana", async () => {
    let push: ((s: unknown) => void) | undefined;
    const ok = { kind: "unavailable", endpoint: null, reason: "x" };
    const bridge = { snapshot: async () => ok, onSnapshot: async (cb: (s: unknown) => void) => (push = cb) } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    push?.({ kind: "connected" });
    expect(store.desk.trouble).toBe("no-agent");
    expect(store.desk.troubleText).toContain("does not understand");
  });
});

describe("emparejar y permisos", () => {
  it("el asistente se abre solo en el paso de emparejar cuando aparece el código", async () => {
    vi.useFakeTimers();
    const store = new Store(new DemoSource(demoOptions("demo&still&peer=none")));
    await store.start();
    expect(store.wizardOpen).toBe(false);
    await store.pairAction("start_pairing");
    expect(store.wizardOpen).toBe(true);
    expect(store.wizardStep).toBe(1);
    vi.advanceTimersByTime(1500);
    expect(store.desk.pair.pairing).toMatchObject({ kind: "comparing", code: "418027" });
    await store.pairAction("confirm_pairing");
    vi.advanceTimersByTime(1300);
    expect(store.desk.pair.peer).toMatchObject({ kind: "paired", name: "PC-Desktop" });
    store.stop();
  });

  it("la bandeja no abre el asistente", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still&peer=none")), null, TRAY_TOPICS);
    await store.start();
    await store.pairAction("start_pairing");
    expect(store.wizardOpen).toBe(false);
    store.stop();
  });

  it("primer arranque: en macOS abre en permisos; en Windows, sin par, abre en «Your desk»", async () => {
    const mac = new Store(new DemoSource(demoOptions("demo&still&setup")));
    await mac.start();
    await mac.maybeFirstRun();
    expect([mac.wizardOpen, mac.wizardStep]).toEqual([true, 2]);
    const win = new Store(new DemoSource(demoOptions("demo&still&setup&here=b")));
    await win.start();
    await win.maybeFirstRun();
    // Con par ya hecho no hay nada que ofrecer.
    expect(win.wizardOpen).toBe(false);
    const fresh = new Store(new DemoSource(demoOptions("demo&still&setup&here=b&peer=none&demo=win-win")));
    await fresh.start();
    await fresh.maybeFirstRun();
    expect([fresh.wizardOpen, fresh.wizardStep]).toEqual([true, 1]);
    // Una vez terminado, no vuelve a abrirse solo.
    await fresh.finishSetup();
    fresh.wizardOpen = false;
    await fresh.maybeFirstRun();
    expect(fresh.wizardOpen).toBe(false);
  });

  it("la fuente real pasa los pedidos al agente y lee los permisos de macOS", async () => {
    const act = vi.fn(async () => ({ kind: "done" as const }));
    const openSettings = vi.fn(async () => {});
    const snap = {
      kind: "connected",
      agent: "a",
      desk: { machines: { a: null, b: null }, screens: [], arrangement: null, scenes: [] },
      state: { on: { a: true, b: false } },
      peer: { kind: "unpaired" },
      pairing: { kind: "idle" },
      monitors: [],
      input: { server: "a", hub_here: null, session: false, locked: false, missing: ["accessibility"], problem: null, defined: false },
      caps: {
        here: {
          machine: "a",
          name: "Estudio",
          os: "macos",
          color: "#7aa7e6",
          hub: null,
          input: { capture: false, inject: false, missing: ["accessibility"], process: { exe: "/x/tandemd", pid: 7, started: 1 } },
          power: { wake_peer: true, wakeable: true, power_off: false },
          services: [],
        },
        peer: null,
      },
    };
    const bridge = { snapshot: async () => snap, onSnapshot: async () => {}, act, openSettings } as unknown as Bridge;
    const src = new LiveSource(bridge);
    const store = new Store(src);
    await store.start();
    expect(store.desk.permissions.needed).toBe(true);
    expect(store.desk.permissions.rows.map((r) => `${r.id}:${r.state}`)).toEqual(["accessibility:missing", "input_monitoring:granted", "local_network:unknown"]);
    expect(store.desk.permissions.binary).toContain("/x/tandemd (pid 7)");
    await store.pairAction("start_pairing");
    expect(act).toHaveBeenLastCalledWith({ kind: "start_pairing" });
    await store.requestPermissions();
    expect(act).toHaveBeenLastCalledWith({ kind: "request_permissions" });
    await store.openPermission("accessibility");
    expect(openSettings).toHaveBeenCalledWith("accessibility");
    act.mockResolvedValueOnce({ kind: "rejected", reason: "ya hay uno in progress" } as never);
    expect(await store.pairAction("start_pairing")).toEqual({ ok: false, reason: "ya hay uno in progress" });
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
    expect(off.troubleText).toBe("PC-Desktop is off");
    expect(off.machines.b?.power).toBe("off");
    expect(off.link.kind).toBeNull();
    const down = demoDesk(demoOptions("demo&trouble=link-down"));
    expect(down.machines.b?.power).toBe("lost");
    const perms = demoDesk(demoOptions("demo&trouble=permissions"));
    expect(perms.permissions.rows.filter((r) => r.state === "missing").map((r) => r.id)).toEqual(["accessibility", "input_monitoring"]);
    expect(demoDesk(demoOptions("demo&trouble=no-agent")).troubleText).toContain("agent");
    expect(demoOptions("demo&trouble=otra").trouble).toBe("none");
  });
});

describe("red en el demo (T-102)", () => {
  it("los caminos de V3-Network: principal, no cable y respaldo, con 60 s y el cambio a Wi-Fi", () => {
    const n = demoDesk(demoOptions("demo&still")).network;
    expect(n.paths.map((p) => [p.kind, p.state])).toEqual([
      ["usb4", "primary"],
      ["ethernet", "unplugged"],
      ["wifi", "backup"],
    ]);
    const usb = n.paths[0]!;
    expect([usb.rttMs, usb.inputP50Ms, usb.inputP99Ms, usb.targetMs, usb.gbps]).toEqual([0.3, 0.4, 1.9, 4, 9.4]);
    expect(n.paths.every((p) => p.history.length === 60)).toBe(true);
    expect(n.paths[2]?.targetMs).toBe(20);
    expect(n.switches[1]).toMatchObject({ from: "usb4", to: "wifi", tookMs: 600 });
    expect(n.switches[0]?.at).toBeGreaterThan(n.switches[1]!.at);
    // El pico de la gráfica es del Wi-Fi, mientras el cable estuvo fuera (corte hace 36 s).
    const wifi = n.paths[2]!.history;
    const peak = Math.max(...wifi.map((h) => h.inputP99Ms ?? 0));
    expect(peak).toBe(5.6);
    expect(wifi.findIndex((h) => h.inputP99Ms === peak)).toBe(27);
    expect(DEMO_NOW - n.switches[1]!.at).toBe(36_000);
    expect(n.now).toBe(DEMO_NOW);
    expect(n.paths.map((p) => p.detail)).toEqual(["USB4 · 20 Gbps", "2.5 GbE", "Wi-Fi 7 · home"]);
    // No link, todos no cable.
    const off = demoDesk(demoOptions("demo&trouble=peer-off")).network;
    expect(off.paths.every((p) => p.state === "unplugged" && p.history.length === 0)).toBe(true);
  });

  it("el historial solo avanza con la pantalla Red visible", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    const store = new Store(src);
    await store.start();
    const usb = () => store.desk.network.paths[0]!.history.map((h) => h.rttMs).join(",");
    const before = usb();
    vi.advanceTimersByTime(5000);
    expect(usb()).toBe(before);
    store.go({ screen: "red" });
    vi.advanceTimersByTime(1600);
    expect(usb()).not.toBe(before);
    expect(store.desk.network.paths[0]!.history).toHaveLength(60);
    // El que no responde no avanza.
    expect(store.desk.network.paths[1]!.history.every((h) => h.rttMs === null)).toBe(true);
  });

  it("medir cualquier camino que responde", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = new Store(src);
    await store.start();
    expect(await src.measureLink("wifi")).toEqual({ ok: true, text: "Wi-Fi: 410 Mbps" });
    expect(store.desk.network.paths[2]?.gbps).toBe(0.41);
    expect(await src.measureLink()).toEqual({ ok: true, text: "USB-C: 9.4 Gbps" });
    expect((await src.measureLink("ethernet")).ok).toBe(false);
  });
});

describe("prueba de cruce en modo demo", () => {
  it("«Test crossing» está disponible y el cruce sale por el borde del acomodo (y el equivocado con cross=wrong)", async () => {
    for (const [opts, same] of [["demo&cross=ok", true], ["demo&cross=wrong", false]] as const) {
      const src = new DemoSource(demoOptions(opts));
      const store = new Store(src);
      await store.start();
      expect(src.can("crossing")).toBe(true);
      const got = await new Promise<{ edge: string }>((resolve) => {
        void store.watchCrossing((s) => resolve(s));
      });
      const { arrangement } = store.desk.layout;
      const expected = exitSide(arrangement, store.desk.here);
      expect(got.edge === expected).toBe(same);
    }
  });
});

describe("fuente real", () => {
  const connected = (over: Partial<Extract<Snapshot, { kind: "connected" }>> = {}): Snapshot => ({
    kind: "connected",
    agent: "b",
    desk: { machines: { a: { os: "windows", name: "Taller", color: "#6cc59a" }, b: null }, screens: [], arrangement: { a_side: "up", align: "end" }, scenes: [] },
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
    monitors: [],
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

  it("sin [[screens]], el acomodo usa las pantallas que lee el motor de cada máquina, a escala entre sí", () => {
    const snap = connected({
      agent: "a",
      input: {
        server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true,
        screens: [{ id: "m32", x: 0, y: 0, width: 1920, height: 1080, scale_milli: 1000 }],
        peer_screens: [{ id: "m34", x: 0, y: 0, width: 3440, height: 1440, scale_milli: 1500 }],
      },
    });
    if (snap.kind !== "connected") throw new Error("foto");
    const { screens } = layoutFromDesk(snap);
    const a = screens.find((s) => s.machine === "a");
    const b = screens.find((s) => s.machine === "b");
    expect(a).toMatchObject({ res: "1920 × 1080", ratio: "16:9", scale: "100 %" });
    expect(b).toMatchObject({ res: "3440 × 1440", ratio: "21:9", scale: "150 %" });
    // 3440 px a 150 % son 2293 puntos: el dibujo respeta esa proporción entre las dos máquinas.
    expect(b!.w / a!.w).toBeCloseTo(3440 / 1.5 / 1920, 3);
    expect(a!.native).toMatchObject({ id: "m32", w: 1920 });
  });

  it("watchCrossing entrega el Crossed real con la entrada medida en la pantalla de llegada", async () => {
    const act = vi.fn(async (_a: unknown) => ({ kind: "done" as const }));
    let notice: ((n: Notice) => void) | undefined;
    const bridge = {
      snapshot: async () => connected({
        agent: "a",
        input: {
          server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true,
          screens: [{ id: "m32", x: 0, y: 0, width: 1920, height: 1080, scale_milli: 1000 }],
          peer_screens: [{ id: "m34", x: 0, y: 0, width: 3440, height: 1440, scale_milli: 1000 }],
        },
      }),
      onSnapshot: async () => {},
      act,
      onNotice: async (cb: (n: Notice) => void) => {
        notice = cb;
      },
    } as unknown as Bridge;
    const src = new LiveSource(bridge);
    await src.start(() => {});
    expect(src.can("crossing")).toBe(true);
    const got: unknown[] = [];
    const stop = await src.watchCrossing((s) => got.push(s));
    expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true });
    notice?.({ kind: "crossed", from: "a", to: "b", edge: "right" });
    notice?.({ kind: "cursor", sample: { machine: "b", screen: "m34", x: 2, y: 600 } });
    expect(got).toEqual([expect.objectContaining({ from: "a", to: "b", edge: "right", at: 600, of: 1440 })]);
    stop();
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_cursor", on: false });
  });

  describe("watchCrossing sin muestra de llegada (reloj falso)", () => {
    async function started() {
      vi.useFakeTimers();
      let notice: ((n: Notice) => void) | undefined;
      const bridge = {
        snapshot: async () => connected({ agent: "a" }),
        onSnapshot: async () => {},
        act: async () => ({ kind: "done" as const }),
        onNotice: async (cb: (n: Notice) => void) => {
          notice = cb;
        },
      } as unknown as Bridge;
      const src = new LiveSource(bridge);
      await src.start(() => {});
      const got: { from: string; edge: string; at: number; of: number; ms: number | null }[] = [];
      await src.watchCrossing((s) => got.push(s));
      return { got, notice: (n: Notice) => notice?.(n) };
    }
    afterEach(() => vi.useRealTimers());

    it("un crossed sin cursor se informa a los 250 ms con of 0 y sin latencia inventada", async () => {
      const { got, notice } = await started();
      notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
      vi.advanceTimersByTime(249);
      expect(got).toEqual([]);
      vi.advanceTimersByTime(2);
      expect(got).toEqual([expect.objectContaining({ from: "a", edge: "left", at: 0, of: 0, ms: null })]);
    });

    it("un crossed nuevo reemplaza al pendiente: solo se informa el último", async () => {
      const { got, notice } = await started();
      notice({ kind: "crossed", from: "a", to: "b", edge: "up" });
      vi.advanceTimersByTime(200);
      notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
      vi.advanceTimersByTime(200);
      expect(got).toEqual([]);
      vi.advanceTimersByTime(100);
      expect(got).toEqual([expect.objectContaining({ edge: "left", ms: null })]);
    });

    it("una muestra de otra máquina no cierra el cruce pendiente", async () => {
      const { got, notice } = await started();
      notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
      notice({ kind: "cursor", sample: { machine: "a", screen: "m32", x: 5, y: 5 } });
      expect(got).toEqual([]);
      vi.advanceTimersByTime(300);
      expect(got).toHaveLength(1);
    });
  });

  it("los estados difíciles salen de la foto", () => {
    expect(deskFromSnapshot({ kind: "connecting" }).ready).toBe(false);
    expect(deskFromSnapshot({ kind: "unavailable", endpoint: null, reason: "no corre" })).toMatchObject({ trouble: "no-agent", troubleText: "no corre" });
    expect(deskFromSnapshot(connected({ state: { on: { a: false, b: true } } }))).toMatchObject({ trouble: "peer-off", troubleText: "Taller is off" });
    expect(deskFromSnapshot(connected({ peer: { kind: "paired", name: "Taller", link: null } })).trouble).toBe("link-down");
    const missing = connected();
    if (missing.kind === "connected") missing.input = { ...missing.input, missing: ["accessibility"] };
    expect(deskFromSnapshot(missing).trouble).toBe("permissions");
  });

  it("T-159: el nombre del puesto y la MAC para encender salen de la foto", () => {
    const snap = connected();
    expect(deskFromSnapshot(snap).deskName).toBe("My desk");
    expect(deskFromSnapshot(snap).machines.a?.wake).toEqual({ mac: null });
    if (snap.kind === "connected") {
      snap.desk = { ...snap.desk, desk_name: " Oficina " };
      snap.caps.here.power = { ...snap.caps.here.power, wol_mac: "aa:bb:cc:dd:ee:ff" };
    }
    const d = deskFromSnapshot(snap);
    expect(d.deskName).toBe("Oficina");
    expect(d.machines.a?.wake).toEqual({ mac: "aa:bb:cc:dd:ee:ff" });
    expect(d.machines.b?.wake).toBeNull();
  });

  it("pide al agente lo que sabe hacer y dice lo que todavía no", async () => {
    const act = vi.fn(async (_a: unknown) => ({ kind: "bandwidth" as const, link: "wifi" as const, bytes_per_sec: 51_250_000 }));
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
    // Con el agente conectado se aplican escenas y se cambian monitores (T-116).
    expect(src.can("scenes")).toBe(true);
    expect(src.can("monitors")).toBe(true);
    expect(src.can("forget")).toBe(false);
    // Sin red a la vista, mide el camino por el que se ve a the other machine (410 Mbps = 51.25 MB/s).
    expect(await src.measureLink()).toEqual({ ok: true, text: "Wi-Fi: 410 Mbps" });
    expect(act).toHaveBeenLastCalledWith({ kind: "measure_link", path: "wifi" });
    expect(await src.measureLink("usb4")).toEqual({ ok: true, text: "USB-C: 410 Mbps" });
    expect(act).toHaveBeenLastCalledWith({ kind: "measure_link", path: "usb4" });
    act.mockResolvedValueOnce({ kind: "accepted", txn: 2 } as never);
    expect(await src.applyScene("game")).toEqual({ ok: true });
    expect(act).toHaveBeenLastCalledWith({ kind: "apply_scene", id: "game" });
    act.mockResolvedValueOnce({ kind: "rejected", reason: "hdmi no signal" } as never);
    expect(await src.switchMonitor("g34", "hdmi")).toEqual({ ok: false, reason: "hdmi no signal" });
    expect(act).toHaveBeenLastCalledWith({ kind: "switch_monitor", monitor: "g34", input: "hdmi" });
    act.mockResolvedValueOnce({ kind: "done" } as never);
    expect(await src.setModule("b", "clipboard", false)).toEqual({ ok: true });
    expect(act).toHaveBeenLastCalledWith({ kind: "set_module", machine: "b", module: "clipboard", on: false });
    act.mockResolvedValueOnce({ kind: "accepted", txn: 1 } as never);
    expect(await src.setPower("a", false)).toEqual({ ok: true });
    expect(act).toHaveBeenLastCalledWith({ kind: "set_power", machine: "a", on: false });
    src.watch(["link"]);
    expect(src.currentTopics).toEqual(["link"]);
  });
});

describe("escenas y transacciones contra el puente simulado", () => {
  const scene: SceneJson = { id: "game", name: "Game", monitors: [["g34", "b"]], shortcut: ["ControlLeft", "AltLeft", "Digit4"] };

  function fake() {
    const act = vi.fn(async (_a: unknown) => ({ kind: "done" as const }));
    let notice: ((n: Notice) => void) | undefined;
    const onNotice = vi.fn(async (cb: (n: Notice) => void) => {
      notice = cb;
    });
    const bridge = { snapshot: async () => ({ kind: "connecting" }) as Snapshot, onSnapshot: async () => {}, act, onNotice } as unknown as Bridge;
    return { src: new LiveSource(bridge), act, onNotice, emit: (event: TxnView) => notice?.({ kind: "txn", event } as Notice) };
  }

  it("saveScene crea con add_scene y edita con edit_scene, y dice el texto de guardado", async () => {
    const { src, act } = fake();
    expect(await src.saveScene(scene, true)).toEqual({ ok: true, text: "Scene “Game” saved" });
    expect(act).toHaveBeenLastCalledWith({ kind: "add_scene", scene });
    expect(await src.saveScene(scene, false)).toEqual({ ok: true, text: "Scene “Game” saved" });
    expect(act).toHaveBeenLastCalledWith({ kind: "edit_scene", scene });
  });

  it("saveScene devuelve la razón si el agente rechaza la escena", async () => {
    const { src, act } = fake();
    act.mockResolvedValueOnce({ kind: "rejected", reason: "el atajo ya lo usa “Work”" } as never);
    expect(await src.saveScene(scene, true)).toEqual({ ok: false, reason: "el atajo ya lo usa “Work”" });
  });

  it("watchTxn oye una sola vez, reparte cada aviso a todos y deja de repartir al cancelar", async () => {
    const { src, onNotice, emit } = fake();
    const a: TxnView[] = [];
    const b: TxnView[] = [];
    const stopA = await src.watchTxn((e) => a.push(e));
    await src.watchTxn((e) => b.push(e));
    expect(onNotice).toHaveBeenCalledTimes(1);
    const started: TxnView = { type: "started", txn: 3, steps: [{ step: "geometry" }] };
    emit(started);
    expect(a).toEqual([started]);
    expect(b).toEqual([started]);
    stopA();
    emit({ type: "step_started", txn: 3, index: 0 });
    expect(a).toHaveLength(1);
    expect(b).toHaveLength(2);
  });

  it("el store separa el encendido de la escena y recuerda el enlace medido", async () => {
    const { src, emit, act } = fake();
    const store = new Store(src);
    await store.start();
    emit({ type: "started", txn: 1, steps: [{ step: "geometry" }] });
    emit({ type: "started", txn: 2, steps: [{ step: "power_on", machine: "b" }] });
    expect(store.run?.progress.txn).toBe(1);
    expect(store.powerRun?.progress.txn).toBe(2);
    emit({ type: "step_started", txn: 2, index: 0 });
    expect(store.powerRun?.progress.status[0]).toBe("run");
    expect(store.run?.progress.status[0]).toBe("pending");
    act.mockResolvedValueOnce({ kind: "bandwidth", link: "wifi", bytes_per_sec: 51_250_000 } as never);
    await store.measureLink("wifi");
    expect(store.measured).toBe("wifi");
  });
});

describe("el enlace del demo sin emparejar", () => {
  it("antes de emparejar solo se ve a the other machine por Wi-Fi; al emparejar pasa al cable", async () => {
    vi.useFakeTimers();
    const store = new Store(new DemoSource(demoOptions("demo&still&peer=none")));
    await store.start();
    expect(store.desk.link.kind).toBe("wifi");
    await store.pairAction("start_pairing");
    await vi.advanceTimersByTimeAsync(2000);
    await store.pairAction("confirm_pairing");
    await vi.advanceTimersByTimeAsync(5000);
    expect(store.desk.pair.peer.kind).toBe("paired");
    expect(store.desk.link.kind).toBe("usb4");
    await store.pairAction("unpair");
    expect(store.desk.link.kind).toBe("wifi");
  });
});
