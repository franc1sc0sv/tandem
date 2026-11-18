// Correcciones de la aceptación (parte B): T-153 (`can()` reactivo), T-154 (túneles al
// abrir), T-157 (estado del bloqueo) y T-158 (módulos con estado real). Puente simulado y demo.
import { act as flush, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { buildCommands } from "../lib/commands";
import { STORE_KEY } from "../lib/context";
import { summaryRows } from "../lib/wizard";
import type { Action, ActionResult, Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store, TRAY_TOPICS } from "../lib/store/store.svelte";
import type { Source } from "../lib/store/types";
import type { ProjectWire } from "../lib/tunnels";
import Shell from "../shell/Shell.svelte";
import Tray from "../Tray.svelte";

afterEach(() => {
  vi.useRealTimers();
});

type Connected = Extract<Snapshot, { kind: "connected" }>;

function snapshot(locked = false, modules: unknown[] = []): Connected {
  return {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: { os: "macos", name: "MacBook Pro", color: "#7aa7e6" }, b: { os: "windows", name: "PC-Desktop", color: "#a99be8" } }, screens: [], arrangement: null, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Desktop", link: "usb4" },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked, missing: [], problem: null, defined: true },
    caps: {
      here: {
        machine: "a",
        name: "MacBook Pro",
        os: "macos",
        color: "#7aa7e6",
        hub: null,
        input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: true },
        services: [],
        modules,
      },
      peer: null,
    },
    monitors: [],
  } as unknown as Connected;
}

const wire: ProjectWire[] = [{ name: "tandem", root: "/src/tandem", tunnels: [{ id: "f00dfacecafe0001", title: "Revisar el PR", origin: "a", target: "b", open: true, created: Date.now() - 60_000, running: [] }] }];

/** Un puente simulado: la primera foto la da `first`; las siguientes se empujan a mano. */
function bridge(first: Snapshot, over: Record<string, unknown> = {}, actions: (a: Action) => ActionResult = () => ({ kind: "done" })) {
  let push: ((s: Snapshot) => void) | undefined;
  const act = vi.fn(async (a: Action) => actions(a));
  const b = {
    snapshot: async () => first,
    onSnapshot: async (cb: (s: Snapshot) => void) => {
      push = cb;
    },
    act,
    shortcuts: async () => ({ bindings: { toggle_window: null }, failed: {}, warning: null, file: null }),
    ...over,
  } as unknown as Bridge;
  return { b, act, push: (s: Snapshot) => push?.(s) };
}

async function open(source: Source, hash: string) {
  const store = new Store(source, { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

/** El chip de estado (los avisos con el mismo texto no cuentan). */
const chip = () => [...document.querySelectorAll(".chip")].find((c) => c.textContent?.includes("Cursor locked here")) ?? null;

const tunnelsReply = (a: Action): ActionResult => (a.kind === "tunnels" ? { kind: "tunnels", tunnels: wire } : { kind: "done" });

describe("T-153: los botones con can() se habilitan al conectar el agente", () => {
  it("«Medir ancho de banda» se habilita cuando llega la primera foto", async () => {
    const { b, push } = bridge({ kind: "connecting" });
    await open(new LiveSource(b), "#/red");
    const medir = () => screen.getByRole("button", { name: /Medir ancho de banda/ });
    expect(medir()).toBeDisabled();
    await flush(async () => push(snapshot()));
    expect(medir()).toBeEnabled();
  });

  it("«Aplicar» de Monitores se habilita al llegar la primera foto", async () => {
    const withScene = { ...snapshot(), desk: { ...snapshot().desk, scenes: [{ id: "t", name: "Work", monitors: [], shortcut: [] }] } } as Snapshot;
    const { b, push } = bridge({ kind: "connecting" });
    await open(new LiveSource(b), "#/monitores");
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
    await flush(async () => push(withScene));
    expect(await screen.findByRole("button", { name: "Apply" })).toBeEnabled();
  });

  it("Acomodo reevalúa can() con cada foto; con el par emparejado «Test crossing» queda habilitado (T-150)", async () => {
    const { b, push } = bridge({ kind: "connecting" });
    const src = new LiveSource(b);
    const store = await open(src, "#/acomodo");
    expect(store.can("monitors")).toBe(false);
    await flush(async () => push(snapshot()));
    // `can()` cambia con la foto (T-153) y, con el par emparejado, el cruce real está disponible (T-150).
    expect(store.can("monitors")).toBe(true);
    expect(store.can("crossing")).toBe(true);
  });

  it("vuelve a deshabilitarse si el agente se pierde", async () => {
    const { b, push } = bridge(snapshot());
    await open(new LiveSource(b), "#/red");
    expect(screen.getByRole("button", { name: /Medir ancho de banda/ })).toBeEnabled();
    await flush(async () => push({ kind: "unavailable", endpoint: null, reason: "sin agente" }));
    expect(screen.getByRole("button", { name: /Medir ancho de banda/ })).toBeDisabled();
  });

  it("en modo demo el botón está habilitado desde el principio", async () => {
    await open(new DemoSource(demoOptions("demo&still")), "#/red");
    expect(screen.getByRole("button", { name: /Medir ancho de banda/ })).toBeEnabled();
  });
});

describe("T-154: la ventana pide los túneles que ya existen", () => {
  it("los muestra sin esperar un evento y los pide una sola vez", async () => {
    const { b, act } = bridge(snapshot(), {}, tunnelsReply);
    const store = await open(new LiveSource(b), "#/tuneles");
    await flush(async () => {});
    expect(act).toHaveBeenCalledWith({ kind: "tunnels" });
    expect(store.desk.tunnels.map((t) => t.title)).toEqual(["Revisar el PR"]);
    expect(await screen.findByText("1 abierto · 1 proyecto")).toBeInTheDocument();
    expect(act.mock.calls.filter(([a]) => a.kind === "tunnels")).toHaveLength(1);
  });

  it("la bandeja y la paleta también los muestran tras abrirse", async () => {
    const running = [{ id: 1, command: "cargo test", pid: 9, started: Date.now(), duration: null, state: { kind: "running" as const } }];
    const busy: ProjectWire[] = [{ ...wire[0], tunnels: [{ ...wire[0]!.tunnels[0]!, running }] } as ProjectWire];
    const reply = (a: Action): ActionResult => (a.kind === "tunnels" ? { kind: "tunnels", tunnels: busy } : { kind: "done" });
    const { b } = bridge(snapshot(), {}, reply);
    const store = new Store(new LiveSource(b), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("1 túnel")).toBeInTheDocument());
    const titles = buildCommands(store.desk, store, false).map((c) => c.title);
    expect(titles).toContain("Ir al túnel «Revisar el PR»");
  });

  it("espera a que haya agente y luego los pide", async () => {
    const { b, act, push } = bridge({ kind: "connecting" }, {}, tunnelsReply);
    const store = await open(new LiveSource(b), "#/tuneles");
    expect(act).not.toHaveBeenCalledWith({ kind: "tunnels" });
    await flush(async () => push(snapshot()));
    await flush(async () => {});
    expect(store.desk.tunnels).toHaveLength(1);
  });

  it("un pedido tardío no pisa lo que llegó por evento", async () => {
    let emit: ((g: ProjectWire[]) => void) | undefined;
    let release: (() => void) | undefined;
    const { b } = bridge(snapshot(), {
      onTunnels: async (cb: (g: ProjectWire[]) => void) => {
        emit = cb;
      },
      act: (a: Action) =>
        a.kind === "tunnels"
          ? new Promise<ActionResult>((res) => {
              release = () => res({ kind: "tunnels", tunnels: [] });
            })
          : Promise.resolve({ kind: "done" }),
    });
    const store = await open(new LiveSource(b), "#/tuneles");
    await flush(async () => emit?.(wire));
    await flush(async () => release?.());
    expect(store.desk.tunnels).toHaveLength(1);
  });

  it("si el agente no contesta con túneles, la ventana sigue vacía sin romperse", async () => {
    const { b } = bridge(snapshot());
    const store = await open(new LiveSource(b), "#/tuneles");
    await flush(async () => {});
    expect(store.desk.tunnels).toEqual([]);
    expect(screen.getAllByText("No open tunnels").length).toBeGreaterThan(0);
  });
});

describe("T-157: «Lock here» se ve y la paleta ofrece solo lo que aplica", () => {
  it("el Puesto muestra el chip mientras el cursor está bloqueado y lo quita al soltar", async () => {
    const { b, push } = bridge(snapshot(false));
    await open(new LiveSource(b), "#/puesto");
    expect(chip()).toBeNull();
    await flush(async () => push(snapshot(true)));
    expect(chip()).not.toBeNull();
    await flush(async () => push(snapshot(false)));
    expect(chip()).toBeNull();
  });

  it("la bandeja muestra el chip mientras está bloqueado y lo quita al soltar", async () => {
    const { b, push } = bridge(snapshot(false));
    const store = new Store(new LiveSource(b), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(store.desk.ready).toBe(true));
    expect(chip()).toBeNull();
    await flush(async () => push(snapshot(true)));
    expect(chip()).not.toBeNull();
    await flush(async () => push(snapshot(false)));
    expect(chip()).toBeNull();
  });

  it("la paleta cambia de orden según el estado", async () => {
    const { b, push } = bridge(snapshot(false));
    const store = await open(new LiveSource(b), "#/puesto");
    const lock = () => buildCommands(store.desk, store, false).filter((c) => c.id.startsWith("lock:")).map((c) => c.title);
    expect(lock()).toEqual(["Lock here"]);
    await flush(async () => push(snapshot(true)));
    expect(lock()).toEqual(["Remove the lock"]);
  });

  it("avisa al bloquear y al soltar, y manda la orden al agente", async () => {
    const { b, act } = bridge(snapshot());
    const store = await open(new LiveSource(b), "#/puesto");
    expect(await store.lockHere(true)).toEqual({ ok: true, text: "Cursor locked here" });
    expect(act).toHaveBeenLastCalledWith({ kind: "lock_here", on: true });
    expect(await store.lockHere(false)).toEqual({ ok: true, text: "Lock removed" });
  });

  it("un rechazo del agente se explica y no cambia el estado", async () => {
    const { b } = bridge(snapshot(), {}, () => ({ kind: "rejected", reason: "this machine no es la que tiene el teclado" }));
    const store = await open(new LiveSource(b), "#/puesto");
    expect(await store.lockHere(true)).toEqual({ ok: false, reason: "this machine no es la que tiene el teclado" });
    expect(store.desk.locked).toBe(false);
  });

  it("en modo demo el chip aparece al bloquear y se quita al soltar", async () => {
    const store = await open(new DemoSource(demoOptions("demo&still")), "#/puesto");
    expect(chip()).toBeNull();
    await flush(async () => void (await store.lockHere(true)));
    expect(chip()).not.toBeNull();
    await flush(async () => void (await store.lockHere(false)));
    expect(chip()).toBeNull();
  });
});

describe("T-158: los módulos muestran el estado que dice el agente", () => {
  const mods = [
    { id: "tunnels", name: "Tunnels", detail: "Recibe túneles de Claude", state: "active", switchable: false },
    { id: "files", name: "Archivos", detail: "Arrastrar archivos al cruzar el borde", state: "active", switchable: false },
  ];

  it("no inventa «Aún no está en esta versión»: usa el estado del agente", async () => {
    const { b } = bridge(snapshot(false, mods));
    const store = await open(new LiveSource(b), "#/maquina/a");
    const views = store.desk.machines.a?.modules ?? [];
    expect(views.map((m) => [m.id, m.state])).toEqual([["tunnels", "active"], ["files", "active"]]);
    expect(document.body.textContent).not.toContain("Aún no está en esta versión");
  });

  it("el resumen del asistente dice que los túneles funcionan, o por qué no", async () => {
    // Con un entorno listo en cada lado, lo único que decide es el estado del módulo.
    const environments = [{ id: "sh", kind: "shell", name: "Shell", default: true, ready: true, detail: "" }];
    const both = (state: string, reason?: string) => ({
      ...snapshot(false, [{ ...mods[0], state, reason }]),
      caps: { ...snapshot().caps, here: { ...snapshot().caps.here, environments, modules: [{ ...mods[0], state, reason }] }, peer: { ...snapshot().caps.here, machine: "b", name: "PC-Desktop", os: "windows", environments, modules: [{ ...mods[0], state, reason }] } },
    }) as unknown as Snapshot;
    const rows = async (snap: Snapshot) => {
      const store = await open(new LiveSource(bridge(snap).b), "#/puesto");
      return summaryRows(store.desk, { crossed: true }).find((r) => r.id === "tunnels");
    };
    expect(await rows(both("active"))).toMatchObject({ ok: true, note: "MCP ready for Claude Code on both" });
    expect(await rows(both("unavailable", "El sistema no dejó abrir nada"))).toMatchObject({ ok: false });
  });

  it("una razón verdadera sí se muestra", async () => {
    const reason = "El sistema no dejó abrir el detector de arrastre de archivos";
    const down = [{ ...mods[1], state: "unavailable", reason }];
    const { b } = bridge(snapshot(false, down));
    const store = await open(new LiveSource(b), "#/maquina/a");
    expect(store.desk.machines.a?.modules[0]).toMatchObject({ state: "unavailable", detail: reason });
  });
});

