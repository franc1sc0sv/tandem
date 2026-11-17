// Página Máquina (T-115): lo que muestra y lo que se puede hacer, en modo demo y contra una
// fuente que habla con el agente (IPC simulada).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Action, ResourceSample, Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import Shell from "../shell/Shell.svelte";
import Maquina from "./Maquina.svelte";

afterEach(() => {
  vi.useRealTimers();
});

async function mount(query = "demo&still", machine: "a" | "b" = "b") {
  const store = new Store(new DemoSource(demoOptions(query)));
  await store.start();
  store.go({ screen: "maquina", machine });
  const r = render(Maquina, { props: { machine }, context: new Map([[STORE_KEY, store]]) });
  await tick();
  return { ...r, store };
}

const res = () => screen.getByRole("region", { name: "Recursos en vivo" });

describe("V3-Maquina (la otra máquina)", () => {
  it("muestra cabecera, recursos agrupados, disco, módulos y entornos", async () => {
    await mount();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("PC-Escritorio");
    expect(screen.getByText("Windows 11 · en línea")).toBeInTheDocument();
    const r = within(res());
    expect(r.getByText("Ryzen 7 · 16 hilos")).toBeInTheDocument();
    expect(r.getByText("64 %")).toBeInTheDocument();
    expect(r.getByText("62 °C")).toBeInTheDocument();
    expect(r.getByText("17.1")).toBeInTheDocument();
    expect(r.getByText("/ 24 GB")).toBeInTheDocument();
    expect(r.getByText("RTX 5080")).toBeInTheDocument();
    expect(r.getByText("VRAM casi llena")).toBeInTheDocument();
    expect(r.getByText("97 %")).toBeInTheDocument();
    expect(r.getByText("71 °C")).toBeInTheDocument();
    expect(r.getByText("ventiladores 58 %")).toBeInTheDocument();
    expect(screen.getByText("412 GB libres de 1.8 TB")).toBeInTheDocument();
    const mods = within(screen.getByRole("region", { name: "Módulos" }));
    expect(mods.getAllByText("activo")).toHaveLength(6);
    const envs = within(screen.getByRole("region", { name: "Entornos" }));
    // El predeterminado (WSL) va primero aunque el agente lo diga segundo.
    const names = envs.getAllByText(/WSL · Ubuntu|PowerShell 7|Docker Desktop/).map((e) => e.textContent);
    expect(names).toEqual(["WSL · Ubuntu 24.04", "PowerShell 7", "Docker Desktop"]);
    expect(envs.getByText("predeterminado")).toBeInTheDocument();
    expect(envs.getByRole("switch", { name: "Permitir ejecución remota" })).toHaveAttribute("aria-checked", "true");
    // G9: sin textos rotos.
    expect(document.body.textContent).not.toMatch(/undefined|NaN|\{\{/);
  });

  it("el color se cambia en el puesto; el de la otra máquina no se puede elegir", async () => {
    const { store } = await mount();
    const group = screen.getByRole("radiogroup", { name: "Color de la máquina" });
    expect(within(group).getByRole("radio", { name: "Violeta" })).toHaveAttribute("aria-checked", "true");
    expect(within(group).getByRole("radio", { name: "Azul" })).toBeDisabled();
    await userEvent.click(within(group).getByRole("radio", { name: "Verde azulado" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.color).toBe("#5fbfb0"));
    expect(within(group).getByRole("radio", { name: "Verde azulado" })).toHaveAttribute("aria-checked", "true");
    // Las flechas saltan el color ocupado (azul, de la MacBook).
    await fireEvent.keyDown(group, { key: "ArrowRight" });
    await vi.waitFor(() => expect(store.desk.machines.b?.color).toBe("#d9a75a"));
    await fireEvent.keyDown(group, { key: "ArrowRight" });
    await vi.waitFor(() => expect(store.desk.machines.b?.color).toBe("#a99be8"));
  });

  it("el nombre se edita en el lugar: Enter guarda, Escape no", async () => {
    const { store } = await mount();
    await userEvent.click(screen.getByRole("button", { name: /PC-Escritorio/ }));
    const input = screen.getByRole("textbox", { name: "Nombre de la máquina" });
    await userEvent.clear(input);
    await userEvent.type(input, "  Estudio {Enter}");
    await vi.waitFor(() => expect(store.desk.machines.b?.name).toBe("Estudio"));
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Estudio");
    await userEvent.click(screen.getByRole("button", { name: /Estudio/ }));
    await userEvent.type(screen.getByRole("textbox", { name: "Nombre de la máquina" }), "XYZ{Escape}");
    expect(store.desk.machines.b?.name).toBe("Estudio");
    // Vacío: no se manda y se avisa por qué.
    await userEvent.click(screen.getByRole("button", { name: /Estudio/ }));
    await userEvent.clear(screen.getByRole("textbox", { name: "Nombre de la máquina" }));
    await userEvent.keyboard("{Enter}");
    expect(store.desk.machines.b?.name).toBe("Estudio");
    expect(store.toasts.at(-1)).toMatchObject({ tone: "bad", text: "El nombre no puede quedar vacío" });
  });

  it("cada módulo que se puede apagar tiene su interruptor; Energía no", async () => {
    const { store } = await mount();
    const mods = within(screen.getByRole("region", { name: "Módulos" }));
    const clip = mods.getByRole("switch", { name: "Portapapeles" });
    expect(clip).toHaveAttribute("aria-checked", "true");
    await userEvent.click(clip);
    await vi.waitFor(() => expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("off"));
    expect(mods.getByRole("switch", { name: "Portapapeles" })).toHaveTextContent("apagado");
    await userEvent.click(mods.getByRole("switch", { name: "Portapapeles" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("active"));
    expect(mods.queryByRole("switch", { name: "Energía" })).toBeNull();
    expect(mods.getByText("Energía").parentElement).toHaveTextContent("activo");
  });

  it("«Permitir ejecución remota» es el módulo de túneles", async () => {
    const { store } = await mount();
    await userEvent.click(screen.getByRole("switch", { name: "Permitir ejecución remota" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.modules.find((m) => m.id === "tunnels")?.state).toBe("off"));
    expect(screen.getByRole("switch", { name: "Permitir ejecución remota" })).toHaveAttribute("aria-checked", "false");
  });

  it("re-escanear muestra que trabaja hasta que vuelve", async () => {
    vi.useFakeTimers();
    const { store } = await mount();
    const spy = vi.spyOn(store, "rescan");
    const btn = screen.getByRole("button", { name: "Volver a escanear" });
    await fireEvent.click(btn);
    expect(spy).toHaveBeenCalledWith("b");
    expect(btn).toHaveAttribute("aria-busy", "true");
    await vi.advanceTimersByTimeAsync(1000);
    expect(btn).toHaveAttribute("aria-busy", "false");
  });

  it("Apagar apaga la otra máquina y queda Encender", async () => {
    const { store } = await mount();
    await userEvent.click(screen.getByRole("button", { name: "Apagar" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("off"));
    expect(screen.getByText("Windows 11 · apagada")).toBeInTheDocument();
    expect(screen.queryByText("64 %")).toBeNull();
    expect(screen.getByRole("button", { name: "Volver a escanear" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Encender" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("on"));
  });
});

describe("F-MaquinaMac (esta máquina)", () => {
  it("sin Apagar; RAM unificada y GPU integrada con su memoria", async () => {
    await mount("demo&still", "a");
    expect(screen.getByText("macOS 26 · Apple M5")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Apagar" })).toBeNull();
    const r = within(res());
    expect(r.getByText("M5 · 12 núcleos")).toBeInTheDocument();
    expect(r.getByText("unificada")).toBeInTheDocument();
    expect(r.getByText("M5 integrada")).toBeInTheDocument();
    expect(r.getByText("Memoria")).toBeInTheDocument();
    expect(r.getByText("de la RAM")).toBeInTheDocument();
    expect(r.queryByText("VRAM casi llena")).toBeNull();
    expect(r.getByText("sin ventilador")).toBeInTheDocument();
    expect(screen.getByText("231 GB libres de 1 TB")).toBeInTheDocument();
    expect(screen.getByText("84 % · conectada")).toBeInTheDocument();
    const envs = within(screen.getByRole("region", { name: "Entornos" }));
    expect(envs.getByText("zsh")).toBeInTheDocument();
    expect(envs.getByText("predeterminado")).not.toHaveClass("chip");
    // El módulo de túneles de esta máquina se apaga desde su fila: sin el interruptor de abajo.
    expect(envs.queryByRole("switch", { name: "Permitir ejecución remota" })).toBeNull();
    expect(screen.getByRole("switch", { name: "Cómputo remoto" })).toHaveAttribute("aria-checked", "true");
  });

  it("el nombre del puesto va junto al logo solo en las páginas de máquina (V3-Maquina)", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/puesto" });
    await store.start();
    render(Shell, { context: new Map([[STORE_KEY, store]]) });
    await tick();
    const nav = screen.getByRole("navigation", { name: "Principal" });
    expect(within(nav).queryByText("Mi puesto")).toBeNull();
    store.go({ screen: "maquina", machine: "b" });
    await tick();
    expect(within(nav).getByText("Mi puesto")).toBeInTheDocument();
  });

  it("si falta un permiso, la fila «Permisos» lo dice y «Abrir ajustes» lo pide", async () => {
    const { store } = await mount("demo&still&trouble=permissions", "a");
    const mods = within(screen.getByRole("region", { name: "Módulos" }));
    const names = mods.getAllByText(/^(Teclado y mouse|Permisos|Portapapeles)$/).map((e) => e.textContent);
    expect(names).toEqual(["Teclado y mouse", "Permisos", "Portapapeles"]);
    expect(mods.getByText("Falta «Monitorización de entrada»: no puede mandar el teclado. Se comprueba solo.")).toBeInTheDocument();
    const spy = vi.spyOn(store, "openPermission");
    await userEvent.click(mods.getByRole("button", { name: "Abrir ajustes" }));
    expect(spy).toHaveBeenCalledWith("input_monitoring");
    // Concedido ese, la fila pasa al que sigue faltando; con todo concedido, desaparece.
    await vi.waitFor(() => expect(mods.getByText(/Falta «Accesibilidad»/)).toBeInTheDocument());
    await userEvent.click(mods.getByRole("button", { name: "Abrir ajustes" }));
    await vi.waitFor(() => expect(mods.queryByText("Permisos")).toBeNull());
  });

  it("la página de la otra máquina no tiene la fila «Permisos»", async () => {
    await mount("demo&still&trouble=permissions", "b");
    expect(screen.queryByText("Permisos")).toBeNull();
  });

  it("con dos Mac, nombres, colores e íconos salen de cada máquina (D2)", async () => {
    await mount("demo=mac-mac&still", "b");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Mac mini");
    expect(screen.getByRole("radio", { name: "Ámbar" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Azul" })).toBeDisabled();
  });
});

describe("animaciones (G7, G8)", () => {
  const css = readFileSync(join(process.cwd(), "src", "screens", "Maquina.svelte"), "utf8");

  it("están las del mockup: entrada de grupos y filas, líneas, barras, aviso y punto", () => {
    for (const k of ["up", "dr", "grow", "wp", "pl"]) expect(css).toMatch(new RegExp(`@keyframes ${k} \\{`));
    expect(css).toMatch(/\.grp \{[^}]*animation: up 0\.45s cubic-bezier\(0\.2, 0\.8, 0\.2, 1\) both/);
    expect(css).toMatch(/\.mod \{[^}]*animation: up 0\.4s ease-out both/);
    expect(css).toMatch(/\.ln \{[^}]*animation: dr 1\.6s ease-out 0\.2s forwards/);
    expect(css).toMatch(/\.warnchip \{[^}]*animation: wp 2\.4s ease-in-out infinite/);
  });

  it("con prefers-reduced-motion las líneas quedan dibujadas (la regla global apaga el resto)", () => {
    expect(css).toMatch(/@media \(prefers-reduced-motion: reduce\) \{\s*\.ln \{\s*stroke-dashoffset: 0;/);
  });
});

describe("contra el agente (IPC)", () => {
  const GIB = 2 ** 30;
  const caps = {
    machine: "b",
    name: "Taller",
    os: "windows",
    color: "#a99be8",
    hub: null,
    input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
    power: { wake_peer: false, wakeable: true, power_off: true },
    services: [],
    modules: [
      { id: "clipboard", name: "Portapapeles", detail: "Texto", state: "active", reason: null, switchable: true },
      { id: "secure_desktop", name: "Pantallas protegidas", detail: "UAC", state: "unavailable", reason: "Falta el servicio de administrador", switchable: false },
      { id: "tunnels", name: "Túneles", detail: "", state: "active", reason: null, switchable: true },
    ],
    environments: [
      { id: "pwsh", kind: "shell", name: "PowerShell 7", default: true, ready: true, detail: "git", gpu: false },
      { id: "wsl:Ubuntu", kind: "wsl", name: "Ubuntu", default: true, ready: true, detail: "uv · cargo", gpu: true },
    ],
    hardware: { cpu: "AMD Ryzen 9 7950X 16-Core Processor", cores: 32, memory: 64 * GIB, gpus: [{ name: "NVIDIA GeForce RTX 4090", vram: 24 * GIB }] },
  };
  const snap = {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: null, b: null }, screens: [], arrangement: null },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "Taller", link: "usb4" },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
    caps: { here: { ...caps, machine: "a", name: "Estudio", os: "macos", color: "#7aa7e6", modules: [], environments: [], hardware: undefined }, peer: caps },
  } as unknown as Snapshot;

  async function mountLive() {
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    let onRes: ((m: "a" | "b", r: ResourceSample) => void) | undefined;
    const bridge = {
      snapshot: async () => snap,
      onSnapshot: async () => {},
      onResources: async (cb: (m: "a" | "b", r: ResourceSample) => void) => {
        onRes = cb;
      },
      act,
    } as unknown as Bridge;
    const store = new Store(new LiveSource(bridge));
    await store.start();
    store.go({ screen: "maquina", machine: "b" });
    render(Maquina, { props: { machine: "b" }, context: new Map([[STORE_KEY, store]]) });
    await tick();
    return { store, act, push: (r: ResourceSample) => onRes?.("b", r) };
  }

  it("la página pide los recursos en vivo y los dibuja con cada muestra", async () => {
    const { act, push } = await mountLive();
    expect(act).toHaveBeenCalledWith({ kind: "watch_resources", on: true });
    expect(screen.getByText("Ryzen 9 7950X · 32 hilos")).toBeInTheDocument();
    expect(screen.getByText("RTX 4090")).toBeInTheDocument();
    push({ cpu: { load: 425, cores: [], temperature: 55 }, memory: { used: 32 * GIB, total: 64 * GIB }, gpus: [], disk: { free: 100 * GIB, total: 500 * GIB } });
    await tick();
    expect(screen.getByText("43 %")).toBeInTheDocument();
    expect(screen.getByText("55 °C")).toBeInTheDocument();
    expect(screen.getByText("100 GB libres de 500 GB")).toBeInTheDocument();
    // Pantallas protegidas no se puede apagar y dice por qué.
    expect(screen.getByText("Falta el servicio de administrador")).toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: "Pantallas protegidas" })).toBeNull();
  });

  it("cada acción va al agente con su comando", async () => {
    const { act } = await mountLive();
    await userEvent.click(screen.getByRole("switch", { name: "Portapapeles" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_module", machine: "b", module: "clipboard", on: false });
    await userEvent.click(screen.getByRole("radio", { name: "Verde azulado" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "b", color: "#5fbfb0" });
    await userEvent.click(screen.getByRole("button", { name: "Volver a escanear" }));
    expect(act).toHaveBeenCalledWith({ kind: "rescan", machine: "b" });
    await userEvent.click(screen.getByRole("button", { name: /Taller/ }));
    await userEvent.type(screen.getByRole("textbox", { name: "Nombre de la máquina" }), "{Control>}a{/Control}Nuevo{Enter}");
    expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "b", name: "Nuevo" });
    await userEvent.click(screen.getByRole("switch", { name: "Permitir ejecución remota" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_module", machine: "b", module: "tunnels", on: false });
    await userEvent.click(screen.getByRole("button", { name: "Apagar" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "b", on: false });
  });

  it("un rechazo del agente se muestra con su razón", async () => {
    const { store, act } = await mountLive();
    act.mockResolvedValueOnce({ kind: "rejected", reason: "ese color ya es el de Estudio" } as never);
    await userEvent.click(screen.getByRole("radio", { name: "Ámbar" }));
    await vi.waitFor(() => expect(store.toasts.at(-1)).toMatchObject({ tone: "bad", text: "ese color ya es el de Estudio" }));
  });

  it("al salir de la página deja de pedir los recursos", async () => {
    const { store, act } = await mountLive();
    store.go({ screen: "red" });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_resources", on: false });
  });
});
