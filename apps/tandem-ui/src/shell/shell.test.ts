import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import { STORE_KEY } from "../lib/context";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import Tray from "../Tray.svelte";
import Shell from "./Shell.svelte";

async function mountShell(query = "demo&still", hash = "#/puesto") {
  const loc = { hash };
  const store = new Store(new DemoSource(demoOptions(query)), loc);
  await store.start();
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return { ...r, store, loc };
}

describe("barra lateral (D16)", () => {
  it("tiene las pantallas, el contador de túneles, una entrada por máquina con su color y Ajustes", async () => {
    await mountShell();
    const nav = screen.getByRole("navigation", { name: "Principal" });
    const links = within(nav).getAllByRole("link").map((a) => a.textContent?.replace(/\s+/g, " ").trim());
    expect(links).toEqual(["Puesto", "Túneles 3", "Monitores", "Acomodo", "Red", "MacBook Pro", "PC-Escritorio", "Ajustes"]);
    expect(within(nav).getByText("Máquinas")).toBeInTheDocument();
    const dot = within(nav).getByRole("link", { name: "PC-Escritorio" }).querySelector(".mdot") as HTMLElement;
    expect(dot.style.background).toBe("rgb(169, 155, 232)");
    // D1: sin tercera máquina.
    expect(within(nav).queryByText(/Agregar/)).toBeNull();
  });

  it("marca la pantalla actual y navega con clic", async () => {
    const { store, loc } = await mountShell();
    const nav = screen.getByRole("navigation", { name: "Principal" });
    expect(within(nav).getByRole("link", { name: "Puesto" })).toHaveAttribute("aria-current", "page");
    await userEvent.click(within(nav).getByRole("link", { name: "Red" }));
    expect(store.route).toEqual({ screen: "red" });
    expect(loc.hash).toBe("#/red");
    expect(within(nav).getByRole("link", { name: "Red" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Red");
    await userEvent.click(within(nav).getByRole("link", { name: "MacBook Pro" }));
    expect(store.route).toEqual({ screen: "maquina", machine: "a" });
  });

  it("con dos Windows, los nombres y colores salen de cada máquina (D2)", async () => {
    await mountShell("demo=win-win&still");
    const nav = screen.getByRole("navigation", { name: "Principal" });
    expect(within(nav).getByRole("link", { name: "PC-Escritorio" })).toBeInTheDocument();
    expect(within(nav).getByRole("link", { name: "PC-Taller" })).toBeInTheDocument();
    expect(within(nav).queryByText("MacBook Pro")).toBeNull();
  });
});

describe("paleta ⌘K / Ctrl+K (D15)", () => {
  it("Ctrl+K la abre con el foco en la búsqueda; Escape la cierra", async () => {
    const { store } = await mountShell();
    await fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    await tick();
    expect(store.paletteOpen).toBe(true);
    const input = screen.getByRole("combobox", { name: "Buscar" });
    await tick();
    expect(input).toHaveFocus();
    await fireEvent.keyDown(input, { key: "Escape" });
    await tick();
    expect(store.paletteOpen).toBe(false);
  });

  it("el botón «Buscar o ejecutar…» también la abre", async () => {
    const { store } = await mountShell();
    await userEvent.click(screen.getByRole("button", { name: /Buscar o ejecutar/ }));
    expect(store.paletteOpen).toBe(true);
  });

  it("filtra al escribir, se mueve con flechas y ejecuta con Enter", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    const input = screen.getByRole("combobox", { name: "Buscar" });
    await user.type(input, "aplicar");
    const options = screen.getAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual(["Aplicar «Trabajo»activa⌃⌥1", "Aplicar «Solo MacBook»⌃⌥2", "Aplicar «Juego»⌃⌥3"]);
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(screen.getAllByRole("option")[2]).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveAttribute("aria-activedescendant", "cmd-scene:juego");
    await user.keyboard("{Enter}");
    await tick();
    expect(store.paletteOpen).toBe(false);
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("juego"));
  });

  it("sin resultados lo dice", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    await user.type(screen.getByRole("combobox", { name: "Buscar" }), "zzz");
    expect(screen.queryAllByRole("option")).toEqual([]);
    expect(screen.getByText("Nada coincide con «zzz».")).toBeInTheDocument();
  });

  it("ir a una pantalla desde la paleta, con clic", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    await user.type(screen.getByRole("combobox", { name: "Buscar" }), "acomodo");
    await user.click(screen.getByRole("option", { name: /Acomodo/ }));
    expect(store.route).toEqual({ screen: "acomodo" });
    expect(store.paletteOpen).toBe(false);
  });

  it("abrir el asistente desde la paleta", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    await user.type(screen.getByRole("combobox", { name: "Buscar" }), "asistente{Enter}");
    await tick();
    expect(store.wizardOpen).toBe(true);
    expect(screen.getByRole("dialog", { name: "Asistente" })).toBeInTheDocument();
  });
});

describe("pantallas", () => {
  it("Puesto muestra las dos máquinas y aplica escenas", async () => {
    const { store } = await mountShell();
    expect(screen.getByRole("link", { name: /MacBook Pro.*macOS · M5/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Juego/ }));
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("juego"));
    expect(screen.getByRole("button", { name: /Juego/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("un estado difícil reemplaza al Puesto y ofrece la acción", async () => {
    const { store } = await mountShell("demo&still&trouble=peer-off");
    expect(screen.getByText("PC-Escritorio está apagada")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Encender PC-Escritorio" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("on"));
  });

  it("los módulos de una máquina se apagan con su interruptor", async () => {
    const { store } = await mountShell("demo&still", "#/maquina/b");
    await userEvent.click(screen.getByRole("switch", { name: "Portapapeles" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("off"));
  });

  it("⌘, / Ctrl+, va a Ajustes", async () => {
    const { store } = await mountShell();
    await fireEvent.keyDown(window, { key: ",", ctrlKey: true });
    expect(store.route).toEqual({ screen: "ajustes" });
  });
});

describe("bandeja (V3-Bandeja)", () => {
  it("muestra las máquinas, aplica escenas, abre la ventana y sale", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    const actions = { showMain: vi.fn(), quit: vi.fn() };
    render(Tray, { store, actions });
    await vi.waitFor(() => expect(screen.getByText("MacBook Pro")).toBeInTheDocument());
    expect(screen.getByText("PC-Escritorio con la VRAM al 88 %")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Juego" }));
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("juego"));
    await userEvent.click(screen.getByRole("button", { name: "Abrir" }));
    await userEvent.click(screen.getByRole("button", { name: "Salir" }));
    expect(actions.showMain).toHaveBeenCalledOnce();
    expect(actions.quit).toHaveBeenCalledOnce();
  });
});
