// Pantalla Tunnels (T-114, `V3-Compute`): lo que muestra y cada interacción, en modo demo y
// contra un agente simulado (IPC). Solo lectura (D9): ningún botón detiene ni abre nada.
import { act as flush, fireEvent, render, screen, within } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Action, ActionResult, Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import type { Source } from "../lib/store/types";
import type { OutputWire, ProcessWire, ProjectWire, TransferWire } from "../lib/tunnels";
import Shell from "../shell/Shell.svelte";

afterEach(() => {
  vi.useRealTimers();
});

async function open(source: Source) {
  const store = new Store(source, { hash: "#/tunnels" });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const demo = (q = "demo&still") => open(new DemoSource(demoOptions(q)));
const detail = () => within(screen.getByRole("region", { name: "Tunnel detail" }));
const list = () => within(screen.getByRole("region", { name: "Tunnels by project" }));
const tunnelButton = (re: RegExp) => list().getByRole("button", { name: re });

describe("Tunnels en modo demo", () => {
  it("dibuja los proyectos con su proporción, el resumen y el primer tunnel", async () => {
    await demo();
    expect(screen.getByRole("heading", { name: "Tunnels", level: 1 })).toBeInTheDocument();
    expect(screen.getByText("4 open · 2 projects")).toBeInTheDocument();
    expect(list().getByRole("button", { name: /^tandem/ })).toHaveAttribute("aria-expanded", "true");
    expect(list().getByText("3/3")).toBeInTheDocument();
    expect(list().getByText("1/2")).toBeInTheDocument();
    expect(detail().getByRole("heading", { level: 2 })).toHaveTextContent("Monitors crate (T-097)");
    expect(detail().getByText("task/T-097")).toBeInTheDocument();
    expect(detail().getByText("WSL · Ubuntu")).toBeInTheDocument();
    expect(detail().getByText("120 MB")).toBeInTheDocument();
    expect(detail().getByText("34 m")).toBeInTheDocument();
    expect(detail().getByText("22")).toBeInTheDocument();
    expect(detail().getByText("0.3 ms")).toBeInTheDocument();
  });

  it("lista solo los procesos que corren y muestra la salida del primero", async () => {
    await demo();
    const procs = detail().getByRole("listbox", { name: "Running processes" });
    expect(within(procs).getAllByRole("option")).toHaveLength(6);
    expect(within(procs).getByText("6 processes running")).toBeInTheDocument();
    expect(within(procs).getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByText(/test hotplug_adds_monitor \.\.\. ok/)).toBeInTheDocument();
    expect(detail().getByText("pid 4127 · 2 m")).toBeInTheDocument();
  });

  it("elegir otro proceso cambia su encabezado y su salida", async () => {
    await demo();
    await fireEvent.click(detail().getAllByRole("option")[1]);
    expect(detail().getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByText(/GET \/api\/monitors 200/)).toBeInTheDocument();
    expect(detail().getByText("pid 3810 · 30 m")).toBeInTheDocument();
    expect(screen.queryByText(/Compiling tandem-monitor/)).toBeNull();
  });

  it("elegir otro tunnel cambia el detalle y la lista de procesos", async () => {
    await demo();
    await fireEvent.click(tunnelButton(/Tunnels in tandemd/));
    expect(tunnelButton(/Tunnels in tandemd/)).toHaveAttribute("aria-pressed", "true");
    expect(detail().getByRole("heading", { level: 2 })).toHaveTextContent("Tunnels in tandemd (T-101)");
    expect(detail().getByText("2 processes running")).toBeInTheDocument();
    expect(detail().getAllByRole("option")).toHaveLength(2);
  });

  it("un tunnel sin procesos dice «Nothing running right now» y uno cerrado, «Tunnel closed»", async () => {
    await demo();
    await fireEvent.click(tunnelButton(/Review PR #66/));
    expect(detail().getByText("Nothing running right now")).toBeInTheDocument();
    expect(detail().queryByRole("listbox")).toBeNull();
    expect(detail().getByText("inactive 9 m")).toBeInTheDocument();
    await fireEvent.click(tunnelButton(/Try ComfyUI workflow/));
    expect(detail().getByText("Tunnel closed")).toBeInTheDocument();
    expect(detail().getByText("closed")).toBeInTheDocument();
    // Cerrado: sin tiempo abierto ni latencia.
    expect(detail().getAllByText("—")).toHaveLength(2);
  });

  it("plegar un proyecto esconde sus tunnels y desplegarlo los devuelve", async () => {
    await demo();
    const header = list().getByRole("button", { name: /^tandem/ });
    await fireEvent.click(header);
    expect(list().getByRole("button", { name: /^tandem/ })).toHaveAttribute("aria-expanded", "false");
    expect(list().queryByRole("button", { name: /Tunnels en tandemd/ })).toBeNull();
    // El otro proyecto sigue desplegado.
    expect(tunnelButton(/Train style LoRA/)).toBeInTheDocument();
    await fireEvent.click(list().getByRole("button", { name: /^tandem/ }));
    expect(tunnelButton(/Tunnels in tandemd/)).toBeInTheDocument();
  });

  it("con 12 procesos la lista los trae todos con su encabezado, en una caja con scroll", async () => {
    await demo("demo&still&tunnels=many");
    const procs = detail().getByRole("listbox", { name: "Running processes" });
    expect(within(procs).getAllByRole("option")).toHaveLength(12);
    expect(within(procs).getByText("12 processes running")).toBeInTheDocument();
    expect(detail().getByText("12", { selector: "b" })).toBeInTheDocument();
  });

  it("sin tunnels solo dice que no hay", async () => {
    await demo("demo&still&tunnels=none");
    expect(screen.getAllByText("No open tunnels").length).toBeGreaterThan(0);
    expect(screen.queryByRole("region", { name: "Tunnel detail" })).toBeNull();
  });

  it("es de solo lectura: ningún botón detiene, cierra ni abre tunnels o procesos", async () => {
    await demo();
    const names = within(screen.getByRole("main")).getAllByRole("button").map((b) => b.textContent ?? "");
    expect(names.join("|")).not.toMatch(/detener|cerrar|matar|kill|abrir|nuevo/i);
  });

  it("«How Claude opens them» copia el comando del servidor MCP", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    await demo();
    await fireEvent.click(screen.getByRole("button", { name: "How Claude opens them" }));
    expect(writeText).toHaveBeenCalledWith("tandem mcp");
    expect(await screen.findByText("Copied: tandem mcp")).toBeInTheDocument();
  });

  it("la barra lateral cuenta los tunnels activos y, dentro de la pantalla, los marca con un punto vivo", async () => {
    const store = await demo();
    const nav = within(screen.getByRole("navigation", { name: "Main" }));
    const link = nav.getByRole("link", { name: /Tunnels/ });
    expect(link.querySelector(".pdot")).not.toBeNull();
    expect(store.activeTunnels).toBe(3);
    store.go({ screen: "desk" });
    await tick();
    const away = within(screen.getByRole("navigation", { name: "Main" })).getByRole("link", { name: /Tunnels/ });
    expect(away).toHaveTextContent("3");
    expect(away.querySelector(".pdot")).toBeNull();
  });
});

// ---------------------------------------------------------------------------------------
// Con el agente de verdad: los eventos y consultas de la IPC en lugar del demo.

function snapshot(): Snapshot {
  return {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: { os: "macos", name: "MacBook Pro", color: "#7aa7e6" }, b: { os: "windows", name: "PC-Desktop", color: "#a99be8" } }, screens: [], arrangement: null, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Desktop", link: "usb4" },
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

// Un id de 64 bits como los reales: no cabe en un número de JavaScript.
const ID = "f00dfacecafe0001";
const proc = (id: number, command: string, started: number): ProcessWire => ({ id, command, pid: 900 + id, started, duration: null, state: { kind: "running" } });
const wire = (running: ProcessWire[], open = true): ProjectWire[] =>
  open ? [{ name: "tandem", root: "/src/tandem", tunnels: [{ id: ID, title: "Revisar el PR", origin: "a", target: "b", open: true, created: Date.now() - 5 * 60_000 - 5_000, running }] }] : [];

function bridge(outputs: OutputWire[] = []) {
  const handlers: { tunnels?: (g: ProjectWire[]) => void; transfer?: (t: TransferWire) => void } = {};
  let call = 0;
  const act = vi.fn(async (a: Action): Promise<ActionResult> => {
    if (a.kind === "processes") return { kind: "processes", processes: [proc(1, "cargo test", Date.now() - 2 * 60_000), proc(2, "pnpm dev", Date.now() - 60_000)] };
    if (a.kind === "output") {
      const o = outputs[Math.min(call, outputs.length - 1)];
      call += 1;
      return o ? { kind: "output", output: o } : { kind: "rejected", reason: "sin salida" };
    }
    return { kind: "done" };
  });
  const b = {
    snapshot: async () => snapshot(),
    act,
    onSnapshot: async () => {},
    onTunnels: async (cb: (g: ProjectWire[]) => void) => {
      handlers.tunnels = cb;
    },
    onTransfer: async (cb: (t: TransferWire) => void) => {
      handlers.transfer = cb;
    },
    shortcuts: async () => ({ bindings: { toggle_window: null }, failed: {}, warning: null, file: null }),
  } as unknown as Bridge;
  return { b, act, handlers };
}

const out = (stdout: string, next: number, stderr = ""): OutputWire => ({ process: proc(1, "cargo test", 0), stdout, stderr, next_stdout: next, next_stderr: stderr.length, done: false });

describe("Tunnels con el agente real (IPC)", () => {
  it("pinta los tunnels que manda el agente con los nombres y colores de cada machine", async () => {
    const { b, handlers } = bridge([out("listo\n", 6)]);
    await open(new LiveSource(b));
    expect(screen.getAllByText("No open tunnels").length).toBeGreaterThan(0);
    await flush(async () => handlers.tunnels?.(wire([proc(1, "cargo test", 0)])));
    expect(await screen.findByText("1 open · 1 project")).toBeInTheDocument();
    expect(detail().getByRole("heading", { level: 2 })).toHaveTextContent("Revisar el PR");
    expect(detail().getByText("MacBook Pro")).toBeInTheDocument();
    expect(detail().getByText("PC-Desktop")).toBeInTheDocument();
    expect(detail().getByText("5 m")).toBeInTheDocument();
    // El agente aún no da la rama ni los puertos: no se muestran ni se inventan.
    expect(document.querySelector(".br")).toBeNull();
    expect(detail().queryByText("Ports")).toBeNull();
  });

  it("pide procesos y salida con el id de texto, y la salida nueva se suma a la anterior", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { b, act, handlers } = bridge([out("uno\n", 4), out("dos\n", 8), out("", 8)]);
    await open(new LiveSource(b));
    await flush(async () => handlers.tunnels?.(wire([proc(1, "cargo test", 0)])));
    await flush(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(act).toHaveBeenCalledWith({ kind: "processes", tunnel: ID });
    expect(act).toHaveBeenCalledWith({ kind: "output", tunnel: ID, process: 1, stdout: 0, stderr: 0 });
    expect(await screen.findByText(/uno/)).toBeInTheDocument();
    await flush(async () => {
      await vi.advanceTimersByTimeAsync(1100);
    });
    // La segunda consulta sigue desde donde quedó la primera.
    expect(act).toHaveBeenCalledWith({ kind: "output", tunnel: ID, process: 1, stdout: 4, stderr: 0 });
    expect(document.querySelector(".pbody")?.textContent).toContain("uno\ndos\n");
  });

  it("la salida de error se muestra junto a la normal", async () => {
    const { b, handlers } = bridge([out("bien\n", 5, "ojo\n")]);
    await open(new LiveSource(b));
    await flush(async () => handlers.tunnels?.(wire([proc(1, "cargo test", 0)])));
    expect(await screen.findByText(/bien/)).toBeInTheDocument();
    expect(document.querySelector(".pbody")?.textContent).toContain("ojo");
  });

  it("un tunnel que deja de estar en la lista pasa a «cerrado» y conserva su título", async () => {
    const { b, handlers } = bridge([out("x", 1)]);
    await open(new LiveSource(b));
    await flush(async () => handlers.tunnels?.(wire([proc(1, "cargo test", 0)])));
    expect(await screen.findByText("1 open · 1 project")).toBeInTheDocument();
    await flush(async () => handlers.tunnels?.(wire([], false)));
    expect((await screen.findAllByText("No open tunnels")).length).toBeGreaterThan(0);
    expect(detail().getByRole("heading", { level: 2 })).toHaveTextContent("Revisar el PR");
    expect(detail().getByText("Tunnel closed")).toBeInTheDocument();
    expect(detail().getAllByText("closed").length).toBeGreaterThan(0);
  });

  it("las copias de archivos suman al tráfico del tunnel", async () => {
    const { b, handlers } = bridge([out("x", 1)]);
    await open(new LiveSource(b));
    await flush(async () => handlers.tunnels?.(wire([proc(1, "cargo test", 0)])));
    expect(await screen.findByText("1 open · 1 project")).toBeInTheDocument();
    await flush(async () => handlers.transfer?.({ tunnel: ID, upload: true, path: "/a", done: 2_500_000, total: 5_000_000, finished: false }));
    expect(await screen.findByText("2.5 MB")).toBeInTheDocument();
  });

  it("si el agente rechaza la consulta de procesos la pantalla sigue viva", async () => {
    const { b, act, handlers } = bridge();
    act.mockImplementation(async () => ({ kind: "rejected", reason: "tunnel desconocido" }));
    await open(new LiveSource(b));
    await flush(async () => handlers.tunnels?.(wire([])));
    expect(await screen.findByText("1 open · 1 project")).toBeInTheDocument();
    expect(detail().getByText("Nothing running right now")).toBeInTheDocument();
  });
});
