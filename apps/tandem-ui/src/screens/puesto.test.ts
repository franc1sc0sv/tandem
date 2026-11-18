// Pantalla Puesto (T-113): lo que muestra y lo que se puede hacer, en modo demo y contra una
// fuente que habla con el agente (IPC simulada).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Action, Notice, Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import { setReducedMotion } from "../test/setup";
import Puesto from "./Puesto.svelte";

afterEach(() => {
  vi.useRealTimers();
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
});

async function mount(query = "demo&still") {
  const store = new Store(new DemoSource(demoOptions(query)));
  await store.start();
  const r = render(Puesto, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return { ...r, store };
}

const dot = (c: HTMLElement) => c.querySelector<HTMLElement>(".curdot");

describe("V3-Puesto en modo demo", () => {
  it("muestra las dos máquinas, el enlace, los escritorios y las escenas", async () => {
    await mount();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Puesto");
    const machines = within(screen.getByRole("region", { name: "Máquinas" }));
    expect(machines.getByRole("link", { name: /MacBook Pro/ })).toHaveAttribute("href", "#/maquina/a");
    expect(machines.getByRole("link", { name: /PC-Escritorio/ })).toHaveAttribute("href", "#/maquina/b");
    expect(screen.getByRole("link", { name: /Enlace entre MacBook Pro y PC-Escritorio: USB-C/ })).toHaveAttribute("href", "#/red");
    expect(screen.getByText("9.4 Gbps · 0.3 ms")).toBeInTheDocument();
    expect(screen.getByText("p99 1.9 ms")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /A escala: MacBook Pro y 34" · PC-Escritorio/ })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { pressed: true })).toHaveLength(1);
    expect(screen.getByRole("button", { name: /Trabajo/, pressed: true })).toBeInTheDocument();
  });

  it("aplicar una escena la marca como activa", async () => {
    await mount();
    await userEvent.click(screen.getByRole("button", { name: /Solo MacBook/ }));
    expect(screen.getByRole("button", { name: /Solo MacBook/, pressed: true })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Trabajo/, pressed: false })).toBeInTheDocument();
  });

  it("los botones de ícono llevan a acomodo y a monitores", async () => {
    await mount();
    expect(screen.getByRole("link", { name: "Editar acomodo" })).toHaveAttribute("href", "#/acomodo");
    expect(screen.getByRole("link", { name: "Nueva escena" })).toHaveAttribute("href", "#/monitores");
    expect(screen.getByRole("link", { name: "Editar escenas" })).toHaveAttribute("href", "#/monitores");
  });

  it("apagar y encender la otra máquina desde su tarjeta de energía", async () => {
    await mount();
    const card = within(screen.getByRole("group", { name: "Energía de PC-Escritorio" }));
    await userEvent.click(card.getByRole("button", { name: "Apagar" }));
    expect(await card.findByRole("button", { name: "Encender" })).toBeEnabled();
    await userEvent.click(card.getByRole("button", { name: "Encender" }));
    expect(await card.findByRole("button", { name: "Apagar" })).toBeInTheDocument();
  });

  it("no ofrece energía si la otra máquina no está emparejada o ya está perdida", async () => {
    await mount("demo&still&trouble=link-down");
    expect(screen.queryByRole("group", { name: "Energía de PC-Escritorio" })).toBeNull();
  });

  it("el punto del cursor aparece con la primera muestra y el borde no destella solo", async () => {
    const { container } = await mount();
    await vi.waitFor(() => expect(dot(container)).not.toBeNull());
    expect(dot(container)?.style.left).toMatch(/^16(\.\d+)?%$/);
    expect(container.querySelector(".edge.on")).toBeNull();
  });

  it("con otras máquinas (D2) los nombres salen de cada una", async () => {
    await mount("demo=win-win&still");
    expect(screen.getByRole("img", { name: /A escala:/ }).getAttribute("aria-label")).not.toContain("MacBook");
  });
});

describe("contra el agente (IPC)", () => {
  const machine = (m: "a" | "b", name: string, os: string, color: string) => ({
    machine: m,
    name,
    os,
    color,
    hub: null,
    input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
    power: { wake_peer: true, wakeable: true, power_off: true },
    services: [],
    modules: [],
    environments: [],
  });
  const snap = {
    kind: "connected",
    agent: "a",
    desk: {
      machines: { a: null, b: null },
      screens: [
        { machine: "a", id: "mac", x: 0, y: 0, width: 1512, height: 982, scale: 2 },
        { machine: "b", id: "g34", x: 0, y: 0, width: 3440, height: 1440, scale: 1 },
      ],
      arrangement: { a_side: "left", align: "end" },
      scenes: [],
    },
    monitors: [],
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "Taller", link: "usb4" },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
    caps: { here: machine("a", "Estudio", "macos", "#7aa7e6"), peer: machine("b", "Taller", "windows", "#a99be8") },
  } as unknown as Snapshot;

  async function mountLive(reply: "done" | "rejected" = "done") {
    const act = vi.fn(async (a: Action) => (reply === "rejected" && a.kind === "watch_cursor" ? { kind: "rejected" as const, reason: "no" } : { kind: "done" as const }));
    let notice: ((n: Notice) => void) | undefined;
    const bridge = {
      snapshot: async () => snap,
      onSnapshot: async () => {},
      onResources: async () => {},
      onNotice: async (cb: (n: Notice) => void) => {
        notice = cb;
      },
      act,
    } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    const r = render(Puesto, { context: new Map([[STORE_KEY, store]]) });
    await tick();
    return { ...r, act, send: (n: Notice) => notice?.(n) };
  }

  const watches = (act: ReturnType<typeof vi.fn>) => act.mock.calls.map((c) => c[0]).filter((a: Action) => a.kind === "watch_cursor");

  it("pide el cursor en vivo y dibuja cada muestra sobre su pantalla", async () => {
    const { act, send, container } = await mountLive();
    await vi.waitFor(() => expect(watches(act)).toEqual([{ kind: "watch_cursor", on: true }]));
    expect(dot(container)).toBeNull();
    // El centro de la pantalla del 34" (3440 × 1440).
    send({ kind: "cursor", sample: { machine: "b", screen: "g34", x: 1720, y: 720 } });
    await tick();
    const left = parseFloat(dot(container)?.style.left ?? "NaN");
    const top = parseFloat(dot(container)?.style.top ?? "NaN");
    expect(left).toBeGreaterThan(55);
    expect(left).toBeLessThan(70);
    expect(top).toBeGreaterThan(30);
    expect(top).toBeLessThan(50);
  });

  it("cada cruce destella el borde", async () => {
    const { act, send, container } = await mountLive();
    await vi.waitFor(() => expect(watches(act)).toHaveLength(1));
    expect(container.querySelector(".edge.on")).toBeNull();
    send({ kind: "crossed", from: "a", to: "b", edge: "right" });
    await tick();
    expect(container.querySelector(".edge.on")).not.toBeNull();
  });

  it("solo recibe el cursor con la ventana visible", async () => {
    const { act } = await mountLive();
    await vi.waitFor(() => expect(watches(act)).toHaveLength(1));
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.waitFor(() => expect(watches(act).at(-1)).toEqual({ kind: "watch_cursor", on: false }));
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.waitFor(() => expect(watches(act).at(-1)).toEqual({ kind: "watch_cursor", on: true }));
  });

  it("al salir de la pantalla deja de recibirlo", async () => {
    const { act, unmount } = await mountLive();
    await vi.waitFor(() => expect(watches(act)).toHaveLength(1));
    unmount();
    expect(watches(act).at(-1)).toEqual({ kind: "watch_cursor", on: false });
  });

  it("si el agente no lo ofrece, los escritorios se ven sin punto", async () => {
    const { container } = await mountLive("rejected");
    await tick();
    expect(container.querySelector(".scale")).not.toBeNull();
    expect(dot(container)).toBeNull();
  });

  it("apagar la otra máquina va al agente", async () => {
    const { act } = await mountLive();
    await userEvent.click(screen.getByRole("button", { name: "Apagar" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "b", on: false });
  });
});

describe("animaciones (G7, G8)", () => {
  const css = readFileSync(join(process.cwd(), "src", "screens", "Puesto.svelte"), "utf8");

  it("están las del mockup: pulso del punto, flujo del enlace, destello del borde y movimiento del cursor", () => {
    expect(css).toMatch(/\.on-dot \{[^}]*animation: pl 2s ease-in-out infinite/);
    expect(css).toMatch(/\.flow \{[^}]*animation: fl 1s linear infinite/);
    expect(css).toMatch(/\.flow\.r \{[^}]*animation-direction: reverse/);
    expect(css).toMatch(/\.edge\.on \{[^}]*animation: eg 1s ease-in-out/);
    expect(css).toMatch(/@keyframes eg \{/);
    expect(css).toMatch(/\.curdot \{[^}]*transition:/);
    expect(css).toMatch(/\.scene \{[^}]*transition:[^}]*border-color 0\.25s/);
    expect(css).toMatch(/\.thumb div \{[^}]*transition: background 0\.5s/);
  });

  it("con prefers-reduced-motion el demo no mueve el cursor: manda solo el punto de partida", async () => {
    vi.useFakeTimers();
    setReducedMotion(true);
    const src = new DemoSource(demoOptions("demo"));
    await src.start(() => {});
    const seen: Notice[] = [];
    const stop = await src.watchCursor((e) => seen.push(e));
    await vi.advanceTimersByTimeAsync(5000);
    expect(seen).toHaveLength(1);
    stop();
  });

  it("sin esa preferencia el demo mueve el cursor a unos 30 Hz y avisa los cruces", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    await src.start(() => {});
    const seen: Notice[] = [];
    const stop = await src.watchCursor((e) => seen.push(e));
    await vi.advanceTimersByTimeAsync(7000);
    stop();
    const moves = seen.filter((e) => e.kind === "cursor").length;
    expect(moves).toBeGreaterThan(150);
    expect(moves).toBeLessThan(230);
    const crossed = seen.filter((e) => e.kind === "crossed");
    expect(crossed.length).toBeGreaterThanOrEqual(2);
    expect(crossed[0]).toMatchObject({ from: "a", to: "b", edge: "right" });
  });
});

describe("cursor en vivo (T-140)", () => {
  it("watchCursor se llama una sola vez aunque el demo publique una foto nueva en cada tick", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    const spy = vi.spyOn(src, "watchCursor");
    const store = new Store(src);
    await store.start();
    render(Puesto, { context: new Map([[STORE_KEY, store]]) });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("en 7 s el punto llega al 34\" y el borde destella al cruzar, sin reinicios", async () => {
    vi.useFakeTimers();
    const src = new DemoSource(demoOptions("demo"));
    const store = new Store(src);
    await store.start();
    const { container } = render(Puesto, { context: new Map([[STORE_KEY, store]]) });
    let max = 0;
    let flashed = false;
    for (let i = 0; i < 70; i++) {
      await vi.advanceTimersByTimeAsync(100);
      max = Math.max(max, parseFloat(dot(container)?.style.left ?? "0"));
      flashed ||= container.querySelector(".edge.on") !== null;
    }
    expect(max).toBeGreaterThanOrEqual(60);
    expect(flashed).toBe(true);
  });
});
