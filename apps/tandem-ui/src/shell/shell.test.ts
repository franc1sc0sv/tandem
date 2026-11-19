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
    const nav = screen.getByRole("navigation", { name: "Main" });
    const links = within(nav).getAllByRole("link").map((a) => a.textContent?.replace(/\s+/g, " ").trim());
    expect(links).toEqual(["Desk", "Tunnels 3", "Monitors", "Arrangement", "Network", "MacBook Pro", "PC-Desktop", "Settings"]);
    expect(within(nav).getByText("Machines")).toBeInTheDocument();
    const dot = within(nav).getByRole("link", { name: "PC-Desktop" }).querySelector(".mdot") as HTMLElement;
    expect(dot.style.background).toBe("rgb(169, 155, 232)");
    // D1: sin tercera máquina.
    expect(within(nav).queryByText(/Agregar/)).toBeNull();
  });

  it("marca la pantalla actual y navega con clic", async () => {
    const { store, loc } = await mountShell();
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: "Desk" })).toHaveAttribute("aria-current", "page");
    await userEvent.click(within(nav).getByRole("link", { name: "Network" }));
    expect(store.route).toEqual({ screen: "red" });
    expect(loc.hash).toBe("#/red");
    expect(within(nav).getByRole("link", { name: "Network" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Network");
    await userEvent.click(within(nav).getByRole("link", { name: "MacBook Pro" }));
    expect(store.route).toEqual({ screen: "maquina", machine: "a" });
  });

  it("con dos Windows, los nombres y colores salen de cada máquina (D2)", async () => {
    await mountShell("demo=win-win&still");
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: "PC-Desktop" })).toBeInTheDocument();
    expect(within(nav).getByRole("link", { name: "PC-Workshop" })).toBeInTheDocument();
    expect(within(nav).queryByText("MacBook Pro")).toBeNull();
  });
});

describe("paleta ⌘K / Ctrl+K (D15)", () => {
  it("Ctrl+K la abre con el foco en la búsqueda; Escape la cierra", async () => {
    const { store } = await mountShell();
    await fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    await tick();
    expect(store.paletteOpen).toBe(true);
    const input = screen.getByRole("combobox", { name: "Search" });
    await tick();
    expect(input).toHaveFocus();
    await fireEvent.keyDown(input, { key: "Escape" });
    await tick();
    expect(store.paletteOpen).toBe(false);
  });

  it("el botón «Search or run…» también la abre", async () => {
    const { store } = await mountShell();
    await userEvent.click(screen.getByRole("button", { name: /Search or run/ }));
    expect(store.paletteOpen).toBe(true);
  });

  it("filtra al escribir, se mueve con flechas y ejecuta con Enter", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    const input = screen.getByRole("combobox", { name: "Search" });
    await user.type(input, "apply");
    const options = screen.getAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual(["Apply “Work”active⌃⌥1", "Apply “MacBook only”⌃⌥2", "Apply “Game”⌃⌥3"]);
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(screen.getAllByRole("option")[2]).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveAttribute("aria-activedescendant", "cmd-scene:game");
    await user.keyboard("{Enter}");
    await tick();
    expect(store.paletteOpen).toBe(false);
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("game"));
  });

  it("sin resultados lo dice", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    await user.type(screen.getByRole("combobox", { name: "Search" }), "zzz");
    expect(screen.queryAllByRole("option")).toEqual([]);
    expect(screen.getByText("Nothing matches “zzz”.")).toBeInTheDocument();
  });

  it("ir a una pantalla desde la paleta, con clic", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    await user.type(screen.getByRole("combobox", { name: "Search" }), "arrangement");
    await user.click(screen.getByRole("option", { name: /Arrangement/ }));
    expect(store.route).toEqual({ screen: "acomodo" });
    expect(store.paletteOpen).toBe(false);
  });

  it("abrir el asistente desde la paleta", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    await user.type(screen.getByRole("combobox", { name: "Search" }), "setup{Enter}");
    await tick();
    expect(store.wizardOpen).toBe(true);
    expect(screen.getByRole("dialog", { name: "Setup" })).toBeInTheDocument();
  });
});

describe("pantallas", () => {
  it("Puesto muestra las dos máquinas y aplica escenas", async () => {
    const { store } = await mountShell();
    expect(screen.getByRole("link", { name: /MacBook Pro.*macOS · M5/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Game/ }));
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("game"));
    expect(screen.getByRole("button", { name: /Game/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("un estado difícil reemplaza al Puesto y ofrece la acción", async () => {
    const { store } = await mountShell("demo&still&trouble=peer-off");
    expect(screen.getByText("PC-Desktop is off")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Power on PC-Desktop" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("on"));
  });

  it("los módulos de una máquina se apagan con su interruptor", async () => {
    const { store } = await mountShell("demo&still", "#/maquina/b");
    await userEvent.click(screen.getByRole("switch", { name: "Clipboard" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("off"));
  });

  it("⌘, / Ctrl+, va a Ajustes", async () => {
    const { store } = await mountShell();
    await fireEvent.keyDown(window, { key: ",", ctrlKey: true });
    expect(store.route).toEqual({ screen: "ajustes" });
  });
});

describe("Ajustes → Atajos", () => {
  it("graba una combinación, avisa si no sirve y la guarda", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell("demo&still", "#/ajustes");
    await user.click(screen.getByRole("tab", { name: "Shortcuts" }));
    const combo = await screen.findByRole("button", { name: /Shortcut for “Show or hide Tandem”/ });
    expect(combo).toHaveTextContent("Ctrl+Alt+0");
    await user.click(combo);
    expect(combo).toHaveTextContent("Press…");
    await fireEvent.keyDown(combo, { key: "j", code: "KeyJ" });
    expect(screen.getByRole("alert")).toHaveTextContent("Ctrl, Alt or");
    await fireEvent.keyDown(combo, { key: "j", code: "KeyJ", ctrlKey: true, shiftKey: true });
    expect(combo).toHaveTextContent("Ctrl+Shift+J");
    await vi.waitFor(async () => expect((await store.shortcuts()).bindings.toggle_window).toBe("Ctrl+Shift+KeyJ"));
    expect(store.toasts[0]?.text).toBe("Shortcut saved");
  });
});

describe("bandeja (V3-Tray)", () => {
  it("muestra las máquinas, aplica escenas, abre la ventana y sale", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    const actions = { showMain: vi.fn(), quit: vi.fn() };
    render(Tray, { store, actions });
    await vi.waitFor(() => expect(screen.getByText("MacBook Pro")).toBeInTheDocument());
    expect(screen.getByText("PC-Desktop with VRAM at 88 %")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Game" }));
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("game"));
    await userEvent.click(screen.getByRole("button", { name: "Open" }));
    await userEvent.click(screen.getByRole("button", { name: "Leave" }));
    expect(actions.showMain).toHaveBeenCalledOnce();
    expect(actions.quit).toHaveBeenCalledOnce();
  });
});
