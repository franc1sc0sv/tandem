import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act as flush, fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Action, ActionResult, Snapshot } from "../lib/model";
import type { NetworkWire } from "../lib/network";
import { DEMO_NOW, DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import type { Source } from "../lib/store/types";
import Shell from "../shell/Shell.svelte";

afterEach(() => {
  vi.useRealTimers();
});

async function open(source: Source) {
  const store = new Store(source, { hash: "#/network" });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const demo = (q = "demo&still") => open(new DemoSource(demoOptions(q)));
const articles = () => [...document.querySelectorAll("article")];

describe("Red (V3-Network) en modo demo", () => {
  it("dibuja el mapa: las dos machines con su color y los tres caminos", async () => {
    await demo();
    expect(screen.getByRole("heading", { name: "Network", level: 1 })).toBeInTheDocument();
    const map = screen.getByRole("region", { name: "Link map" });
    expect(within(map).getByText("MacBook Pro")).toBeInTheDocument();
    expect(within(map).getByText("PC-Desktop")).toBeInTheDocument();
    const paths = within(map).getByRole("img", { name: "Three paths: USB-C active at 0.3 ms, Ethernet no cable, Wi-Fi as backup at 3.8 ms" });
    expect(within(paths).getByText("USB-C")).toBeInTheDocument();
    expect(within(paths).getByText("0.3 ms")).toBeInTheDocument();
    expect(within(paths).getByText("Ethernet · no cable")).toBeInTheDocument();
    expect(within(paths).getByText("3.8 ms")).toBeInTheDocument();
    // El halo lleva el color de cada machine.
    const halos = [...map.querySelectorAll<HTMLElement>(".node")].map((n) => n.style.getPropertyValue("--c"));
    expect(halos).toEqual(["#7aa7e6", "#a99be8"]);
    // El camino que fluye es el del cable y el del Wi-Fi; Ethernet va punteado.
    expect([...map.querySelectorAll(".flow")].map((p) => p.classList[1])).toEqual(["usb4", "wifi"]);
    expect(map.querySelectorAll('path[stroke-dasharray="4 7"]')).toHaveLength(1);
  });

  it("los indicadores: latencia p50 y p99, ancho de banda y la gráfica con el pico y su nota", async () => {
    await demo();
    expect(screen.getByText("Keyboard and mouse")).toBeInTheDocument();
    expect(screen.getByText("0.4 ms")).toBeInTheDocument();
    expect(screen.getByText("p99 1.9 ms")).toBeInTheDocument();
    const bw = screen.getByText("Bandwidth").closest<HTMLElement>(".card")!;
    expect(within(bw).getByText("9.4 Gbps")).toBeInTheDocument();
    expect(within(bw).getByText("over USB-C")).toBeInTheDocument();
    expect(screen.getByText("Latency · 60 s")).toBeInTheDocument();
    expect(screen.getByText("target 4 ms")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Latency over the last 60 s: now 1.3 ms, peak of 5.6 ms" })).toBeInTheDocument();
    expect(document.querySelectorAll("circle.pk")).toHaveLength(1);
    expect(screen.getByText("Spike when the cable was unplugged · switched to Wi-Fi in 0.6 s")).toBeInTheDocument();
  });

  it("una tarjeta por camino, con su chip de estado uniforme y sus barras", async () => {
    await demo();
    const [usb, eth, wifi] = articles();
    expect(within(usb!).getByText("USB-C cable")).toBeInTheDocument();
    expect(within(usb!).getByText("USB4 · 20 Gbps")).toBeInTheDocument();
    expect(usb!.querySelector(".st")).toHaveClass("primary");
    expect(within(usb!).getByText("Main")).toBeInTheDocument();
    expect(within(usb!).getByText("0.3 ms")).toBeInTheDocument();
    expect(within(usb!).getByText("9.4 Gbps")).toBeInTheDocument();
    expect(within(usb!).getByRole("meter", { name: "USB-C bandwidth" })).toHaveAttribute("aria-valuenow", "94");
    expect(within(eth!).getByText("Ethernet")).toBeInTheDocument();
    expect(within(eth!).getByText("No cable")).toBeInTheDocument();
    expect(eth!.querySelector(".st")).toHaveClass("unplugged");
    expect(eth!.querySelectorAll(".val")[0]).toHaveTextContent("—");
    expect(within(eth!).queryByRole("meter")).toBeNull();
    expect(within(wifi!).getByText("Wi-Fi 7 · home")).toBeInTheDocument();
    expect(wifi!.querySelector(".st")).toHaveClass("backup");
    expect(within(wifi!).getByText("Backup")).toBeInTheDocument();
    expect(within(wifi!).getByText("410 Mbps")).toBeInTheDocument();
    expect(within(wifi!).getByRole("meter", { name: "Wi-Fi latency" })).toHaveAttribute("aria-valuenow", "38");
  });

  it("no hay textos de relleno ni restos de plantilla", async () => {
    await demo();
    const text = document.body.innerText ?? document.body.textContent ?? "";
    for (const bad of ["{{", "undefined", "NaN", "Add machine", "Simulate", "lorem"]) expect(text).not.toContain(bad);
    // Sin la búsqueda de otras pantallas: V3-Network no la tiene.
    expect(screen.queryByText("Search or run…")).toBeNull();
  });

  it("medir ancho de banda mide el camino principal, avisa el valor y no se puede repetir mientras mide", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const src = new DemoSource(demoOptions("demo"));
    const spy = vi.spyOn(src, "measureLink");
    await open(src);
    const btn = screen.getByRole("button", { name: "Measure bandwidth" });
    expect(btn).toBeEnabled();
    await fireEvent.click(btn);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith(undefined);
    // La prueba dura: el botón se apaga y dice qué camino se mide.
    const busy = await screen.findByRole("button", { name: "Measuring USB-C…" });
    expect(busy).toBeDisabled();
    await flush(async () => {
      await vi.advanceTimersByTimeAsync(1600);
    });
    expect(await screen.findByText("USB-C: 9.4 Gbps")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Measure bandwidth" })).toBeEnabled();
  });

  it("no link con the other machine: los tres caminos no cable y el botón apagado", async () => {
    await demo("demo&still&trouble=link-down");
    expect(screen.getAllByText("No cable")).toHaveLength(2);
    expect(screen.getAllByText("No signal")).toHaveLength(1);
    expect(screen.getByText("Wi-Fi · no signal")).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/Wi-Fi[^.]{0,6}no cable/i);
    expect(screen.getByRole("button", { name: "Measure bandwidth" })).toBeDisabled();
    expect(screen.getByText("no input")).toBeInTheDocument();
    expect(screen.getByText("not measured")).toBeInTheDocument();
    expect(screen.getByText("No samples yet")).toBeInTheDocument();
    expect(document.querySelectorAll("circle.pk")).toHaveLength(0);
    expect(document.querySelectorAll(".flow")).toHaveLength(0);
  });

  it("con la pantalla visible, el historial avanza y la nota sale cuando el corte deja la gráfica", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const store = await open(new DemoSource(demoOptions("demo")));
    expect(screen.getByText(/^Spike when the cable was unplugged/)).toBeInTheDocument();
    await flush(async () => {
      await vi.advanceTimersByTimeAsync(1500 * 27);
    });
    expect(store.desk.network.now).toBeGreaterThan(DEMO_NOW + 24_000);
    expect(screen.queryByText(/^Spike when the cable was unplugged/)).toBeNull();
    expect(screen.getByText("Switched from Wi-Fi to USB-C")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------------------
// Con el agente de verdad: la IPC (el puente) en lugar del demo.

function snapshot(link: "usb4" | "wifi" | null = "usb4"): Snapshot {
  return {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: { os: "macos", name: "MacBook Pro", color: "#7aa7e6" }, b: { os: "windows", name: "PC-Desktop", color: "#a99be8" } }, screens: [], arrangement: null, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Desktop", link },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
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
      },
      peer: null,
    },
    monitors: [],
  };
}

const wire = (over: Partial<NetworkWire> = {}): NetworkWire => ({
  paths: [
    { link: "usb4", state: "primary", rtt_us: 300, input_p50_us: 400, input_p99_us: 1900, input_target_us: 4000, bytes_per_sec: 1_175_000_000, history: [{ rtt_us: 300, input_p99_us: 1900 }] },
    { link: "ethernet", state: "unplugged", rtt_us: null, input_p50_us: null, input_p99_us: null, input_target_us: 4000, bytes_per_sec: null, history: [] },
    { link: "wifi", state: "backup", rtt_us: 3800, input_p50_us: null, input_p99_us: null, input_target_us: 20_000, bytes_per_sec: null, history: [{ rtt_us: 3800, input_p99_us: null }] },
  ],
  switches: [],
  measuring: null,
  ...over,
});

function bridge(measure: ActionResult = { kind: "bandwidth", link: "usb4", bytes_per_sec: 1_175_000_000 }) {
  const handlers: { network?: (w: NetworkWire) => void; snapshot?: (s: Snapshot) => void } = {};
  const act = vi.fn(async (a: Action): Promise<ActionResult> => (a.kind === "measure_link" ? measure : { kind: "done" }));
  const b = {
    snapshot: async () => snapshot(),
    act,
    onSnapshot: async (cb: (s: Snapshot) => void) => {
      handlers.snapshot = cb;
    },
    onNetwork: async (cb: (w: NetworkWire) => void) => {
      handlers.network = cb;
    },
    shortcuts: async () => ({ bindings: { toggle_window: null }, failed: {}, warning: null, file: null }),
  } as unknown as Bridge;
  return { b, act, handlers };
}

describe("Red con el agente real (IPC)", () => {
  it("al abrir la pantalla le pide la red al agente y al salir deja de pedirla", async () => {
    const { b, act } = bridge();
    const store = await open(new LiveSource(b));
    expect(act.mock.calls.filter(([a]) => a.kind === "watch_network").map(([a]) => a)).toEqual([{ kind: "watch_network", on: true }]);
    store.go({ screen: "settings" });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_network", on: false });
    store.go({ screen: "network" });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_network", on: true });
    // Volver a pedir lo mismo no repite el pedido.
    const n = act.mock.calls.length;
    store.go({ screen: "network" });
    expect(act.mock.calls.length).toBe(n);
  });

  it("pinta lo que manda el agente, cada segundo, con los nombres y colores de cada machine", async () => {
    const { b, handlers } = bridge();
    await open(new LiveSource(b));
    // Antes de la primera muestra: los tres no cable.
    expect(screen.getAllByText("No cable")).toHaveLength(2);
    expect(screen.getAllByText("No signal")).toHaveLength(1);
    await flush(() => handlers.network?.(wire({ switches: [] })));
    expect(screen.getByText("Main")).toBeInTheDocument();
    expect(screen.getByText("0.4 ms")).toBeInTheDocument();
    expect(screen.getByText("p99 1.9 ms")).toBeInTheDocument();
    expect(within(screen.getByText("Bandwidth").closest<HTMLElement>(".card")!).getByText("9.4 Gbps")).toBeInTheDocument();
    expect(screen.getByText("Backup")).toBeInTheDocument();
    // Sin detalle del link, la línea no se inventa.
    expect(screen.queryByText(/USB4 ·/)).toBeNull();
    expect(screen.getByText("MacBook Pro", { selector: ".node b" })).toBeInTheDocument();
    // Cae el cable: el agente lo dice y la pantalla lo sigue.
    const cut = wire();
    cut.paths[0] = { ...cut.paths[0]!, state: "down", rtt_us: null, input_p50_us: null, input_p99_us: null, bytes_per_sec: null };
    cut.paths[2] = { ...cut.paths[2]!, state: "primary" };
    cut.switches = [{ at_ms: Date.now() - 1000, from: "usb4", to: "wifi", took_ms: 600 }];
    await flush(() => handlers.network?.(cut));
    expect(screen.getByText("Down")).toBeInTheDocument();
    expect(screen.getByText("USB-C · down")).toBeInTheDocument();
    expect(screen.getByText("The cable was unplugged · switched to Wi-Fi in 0.6 s")).toBeInTheDocument();
  });

  it("«Medir ancho de banda» manda MeasureLink por el camino principal y avisa lo medido", async () => {
    const { b, act, handlers } = bridge();
    await open(new LiveSource(b));
    await flush(() => handlers.network?.(wire()));
    await userEvent.click(screen.getByRole("button", { name: "Measure bandwidth" }));
    expect(act).toHaveBeenCalledWith({ kind: "measure_link", path: "usb4" });
    expect(await screen.findByText("USB-C: 9.4 Gbps")).toBeInTheDocument();
  });

  it("si el agente rechaza la prueba, se dice por qué", async () => {
    const { b, handlers } = bridge({ kind: "rejected", reason: "the input went past its target" });
    await open(new LiveSource(b));
    await flush(() => handlers.network?.(wire()));
    await userEvent.click(screen.getByRole("button", { name: "Measure bandwidth" }));
    expect(await screen.findByText("the input went past its target")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Measure bandwidth" })).toBeEnabled();
  });

  it("si el agente está midiendo (otra ventana), el botón lo dice y espera", async () => {
    const { b, handlers } = bridge();
    await open(new LiveSource(b));
    await flush(() => handlers.network?.(wire({ measuring: "wifi" })));
    expect(screen.getByRole("button", { name: "Measuring Wi-Fi…" })).toBeDisabled();
  });

  it("sin the other machine emparejada no se puede medir", async () => {
    const { b } = bridge();
    (b.snapshot as unknown) = async () => ({ ...snapshot(), peer: { kind: "unpaired" } });
    await open(new LiveSource(b));
    expect(screen.getByRole("button", { name: "Measure bandwidth" })).toBeDisabled();
  });
});

// ---------------------------------------------------------------------------------------
// Animaciones (G7) y prefers-reduced-motion (G8).

const SRC = join(process.cwd(), "src");
const read = (...p: string[]) => readFileSync(join(SRC, ...p), "utf8");
const MOCKUP = readFileSync(join(process.cwd(), "..", "..", "mockups", "v03", "V3-Network.dc.html"), "utf8");

describe("animaciones de V3-Network", () => {
  const files = {
    map: read("screens", "network", "LinkMap.svelte"),
    kpis: read("screens", "network", "Indicators.svelte"),
    cards: read("screens", "network", "PathCard.svelte"),
    card: read("ui", "Card.svelte"),
    bar: read("ui", "Bar.svelte"),
    ring: read("ui", "Ring.svelte"),
    button: read("ui", "Button.svelte"),
  };

  it("cada @keyframes del mockup tiene su equivalente en la app", () => {
    const names = [...MOCKUP.matchAll(/@keyframes (\w+)/g)].map((m) => m[1]).sort();
    expect(names).toEqual(["dr", "fa", "grow", "halo", "pg", "pk", "ring", "up"]);
    // Dónde vive el equivalente de cada uno (los nombres de componente cambian, el efecto no).
    expect(files.card).toContain("@keyframes card-up");
    expect(files.map).toContain("@keyframes halo");
    for (const k of ["fa-usb", "fa-eth", "fa-wifi"]) expect(files.map).toContain(`@keyframes ${k}`);
    expect(files.cards).toContain("@keyframes pg");
    expect(files.bar).toContain("@keyframes bar-grow");
    expect(files.ring).toContain("@keyframes ring-in");
    expect(files.kpis).toContain("@keyframes dr");
    expect(files.kpis).toContain("@keyframes pk");
  });

  it("con las mismas duraciones y curvas", () => {
    expect(files.map).toContain("animation: halo 2.6s ease-in-out infinite");
    expect(files.map).toContain("animation: fa-usb 0.8s linear infinite");
    expect(files.map).toContain("animation: fa-eth 0.8s linear infinite");
    expect(files.map).toContain("animation: fa-wifi 2.4s linear infinite");
    expect(files.card).toContain("card-up 0.45s cubic-bezier(0.2, 0.8, 0.2, 1) both");
    expect(files.ring).toContain("ring-in 1.4s cubic-bezier(0.2, 0.8, 0.2, 1) both");
    expect(files.kpis).toContain("animation: dr 2s ease-out 0.2s forwards");
    expect(files.kpis).toContain("animation: pk 2s ease-in-out infinite");
    expect(files.cards).toContain("animation: pg 1.8s infinite");
    // Entrada escalonada de las tarjetas, como el mockup (0.06, 0.1, 0.14 y 0.18…0.26 s).
    expect(MOCKUP).toContain("animation-delay:.06s");
    expect(read("screens", "Network.svelte")).toContain("0.18 + i * 0.04");
    expect(files.kpis).toContain("delay={0.06}");
    expect(files.kpis).toContain("delay={0.1}");
    expect(files.kpis).toContain("delay={0.14}");
  });

  it("y las transiciones: tarjetas, rótulos, íconos y botón", () => {
    expect(files.card).toMatch(/border-color 0\.25s,\s*transform 0\.25s/);
    expect(files.map).toContain("transition: transform 0.25s");
    expect(files.map).toContain("scale(1.05)");
    expect(files.cards).toContain("transition: transform 0.3s cubic-bezier(0.3, 1.5, 0.5, 1)");
    expect(files.cards).toContain("rotate(-8deg) scale(1.08)");
    expect(files.cards).toContain("translateY(-3px)");
    expect(files.button).toContain("rotate(-25deg) scale(1.1)");
  });

  it("con prefers-reduced-motion se apaga todo: la regla global y la línea de la gráfica ya dibujada", () => {
    const css = read("styles", "global.css");
    const rule = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(rule).toContain("animation: none !important");
    expect(rule).toContain("transition: none !important");
    // La línea parte oculta (dashoffset 900) y la anima `dr`: sin animación tiene que verse entera.
    const reduce = /@media \(prefers-reduced-motion: reduce\) \{\s*\.ln \{\s*stroke-dashoffset: 0;/.test(files.kpis);
    expect(reduce).toBe(true);
    // Ningún JS anima aquí: todo es CSS, que esa regla apaga.
    for (const src of Object.values(files)) expect(src).not.toMatch(/\.animate\(|svelte\/transition/);
  });
});
