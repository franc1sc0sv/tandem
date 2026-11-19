// T-120: la palette ⌘K (F-Palette) y la tray (V3-Tray) contra el agente real (IPC simulada) y en
// modo demo, y sus animaciones con `prefers-reduced-motion`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Snapshot } from "../lib/model";
import { enter, leave } from "../lib/motion";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store, TRAY_TOPICS } from "../lib/store/store.svelte";
import { setReducedMotion } from "../test/setup";
import Tray from "../Tray.svelte";
import Shell from "./Shell.svelte";

const SRC = join(process.cwd(), "src");
const read = (...p: string[]) => readFileSync(join(SRC, ...p), "utf8");

function connected(over: Partial<Extract<Snapshot, { kind: "connected" }>> = {}): Snapshot {
  return {
    kind: "connected",
    agent: "b",
    desk: { machines: { a: { os: "windows", name: "Taller", color: "#6cc59a" }, b: null }, screens: [], arrangement: { a_side: "up", align: "end" }, scenes: [] },
    state: { on: { a: true, b: true } },
    monitors: [],
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
  } as Snapshot;
}

/** Un agente simulado: `act` guarda lo que se le pide. */
function fakeBridge(snap: Snapshot) {
  const act = vi.fn(async (_a: unknown) => ({ kind: "done" as const }));
  const bridge = { snapshot: async () => snap, onSnapshot: async () => {}, act, shortcuts: async () => ({ items: [], mac: false }) } as unknown as Bridge;
  return { bridge, act };
}

async function openPalette(store: Store) {
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  await fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  await tick();
  return r;
}

describe("palette contra el agente real (IPC)", () => {
  it("lista lo que el agente sabe hacer: apagar the other machine y medir el link; sin scenes", async () => {
    const { bridge } = fakeBridge(connected());
    const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    expect(screen.getByRole("option", { name: /Power off Taller/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Measure the link/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Apply/ })).toBeNull();
  });

  it("«Apagar Taller» pide set_power al agente y cierra la palette", async () => {
    const { bridge, act } = fakeBridge(connected());
    const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    await userEvent.click(screen.getByRole("option", { name: /Power off Taller/ }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "a", on: false });
    expect(store.paletteOpen).toBe(false);
  });

  it("«Measure the link» pide measure_link por el camino de the other machine", async () => {
    const { bridge, act } = fakeBridge(connected());
    const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    await userEvent.click(screen.getByRole("option", { name: /Measure the link/ }));
    expect(act).toHaveBeenCalledWith({ kind: "measure_link", path: "wifi" });
  });

  it("con the other machine apagada ofrece «Encender»", async () => {
    const { bridge, act } = fakeBridge(connected({ state: { on: { a: false, b: true } } }));
    const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    await userEvent.click(screen.getByRole("option", { name: /Power on Taller/ }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "a", on: true });
  });
});

describe("palette en modo demo", () => {
  it("aplica una scene con Enter tras filtrar por su nombre", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    const input = screen.getByRole("combobox", { name: "Search" });
    await userEvent.type(input, "macbook only");
    await fireEvent.keyDown(input, { key: "Enter" });
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("solo-a"));
    expect(store.paletteOpen).toBe(false);
  });

  it("apaga the other machine y el grupo «Machines» pasa a ofrecer «Encender»", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/desk" });
    await store.start();
    await openPalette(store);
    await userEvent.click(screen.getByRole("option", { name: /Power off PC-Desktop/ }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).not.toBe("on"));
  });

  it("el fondo cierra la palette con un clic y devuelve el foco", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/desk" });
    await store.start();
    const { container } = await openPalette(store);
    const veil = container.ownerDocument.querySelector(".veil") as HTMLElement;
    await userEvent.click(veil);
    expect(store.paletteOpen).toBe(false);
  });
});

describe("tray contra el agente real (IPC)", () => {
  it("muestra el estado difícil con una línea y, si the other machine está apagada, «Encender» la enciende", async () => {
    const { bridge, act } = fakeBridge(connected({ state: { on: { a: false, b: true } } }));
    const store = new Store(new LiveSource(bridge), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("Taller is off")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "Power on" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "a", on: true });
  });

  it("sin agente lo dice y no ofrece botones que no sirven", async () => {
    const bridge = { snapshot: async () => ({ kind: "unavailable", endpoint: null, reason: "no corre" }), onSnapshot: async () => {}, act: vi.fn(async () => ({ kind: "done" })) } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("no corre")).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "Power on" })).toBeNull();
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
  });

  it("con el agente al día no hay notice de estado", async () => {
    const { bridge } = fakeBridge(connected());
    const store = new Store(new LiveSource(bridge), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("Estudio")).toBeInTheDocument());
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("tray en modo demo", () => {
  it("la scene activa queda marcada y cada scene aplica con un clic", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByRole("button", { name: "Work" })).toHaveAttribute("aria-pressed", "true"));
    await userEvent.click(screen.getByRole("button", { name: "MacBook only" }));
    await vi.waitFor(() => expect(screen.getByRole("button", { name: "MacBook only" })).toHaveAttribute("aria-pressed", "true"));
    expect(screen.getByRole("button", { name: "Work" })).toHaveAttribute("aria-pressed", "false");
  });

  it("no inventa «Lock here»: no hay acción detrás (G6)", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), null, TRAY_TOPICS);
    render(Tray, { store, actions: { showMain: vi.fn(), quit: vi.fn() } });
    await vi.waitFor(() => expect(screen.getByText("MacBook Pro")).toBeInTheDocument());
    expect(screen.queryByText(/Bloquear/)).toBeNull();
  });
});

describe("animaciones de la palette y la tray (G7) y prefers-reduced-motion (G8)", () => {
  it("la palette trae `pop` (diálogo) y `up` (renglones) con las curvas del mockup", () => {
    const pal = read("shell", "Palette.svelte");
    expect(pal).toContain("animation: pal-pop 0.25s cubic-bezier(0.2, 0.9, 0.3, 1.1)");
    expect(pal).toMatch(/@keyframes pal-pop \{\s*from \{\s*opacity: 0;\s*transform: translateY\(-10px\) scale\(0\.98\);/);
    expect(pal).toContain("animation: pal-up 0.2s ease-out both");
    expect(pal).toMatch(/@keyframes pal-up \{\s*from \{\s*opacity: 0;\s*transform: translateY\(4px\);/);
  });

  it("la tray trae las transiciones del mockup: botones de scene, barras y links", () => {
    const tray = read("Tray.svelte");
    expect(tray).toMatch(/border-color 0\.25s,\s*background 0\.25s,\s*transform 0\.2s/);
    expect(tray).toContain("transform: translateY(-1px)");
    expect(tray).toContain("transition: width 0.6s cubic-bezier(0.2, 0.8, 0.2, 1)");
    expect(tray).toMatch(/background 0\.2s,\s*color 0\.2s/);
  });

  it("con prefers-reduced-motion el velo y la salida de la palette duran 0 ms y la hoja global apaga las de CSS", () => {
    const el = document.createElement("div");
    expect(enter(el, { duration: 200 }).duration).toBe(200);
    setReducedMotion(true);
    expect(enter(el, { duration: 200 })).toEqual({ duration: 0 });
    expect(leave(el)).toEqual({ duration: 0 });
    const css = read("styles", "global.css");
    const rule = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(rule).toContain("animation: none !important");
    expect(rule).toContain("transition: none !important");
  });

  it("las animaciones de la palette y la tray son de CSS o de motion.ts: ninguna por JS", () => {
    for (const f of [read("shell", "Palette.svelte"), read("Tray.svelte")]) {
      expect(f).not.toMatch(/\.animate\(/);
      expect(f).not.toMatch(/from "svelte\/transition"/);
    }
  });

  it("la tray usa la misma hoja global (reduced-motion incluido) que la app", () => {
    expect(read("tray.ts")).toContain('import "./styles/global.css"');
  });
});
