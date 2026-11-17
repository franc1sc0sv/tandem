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

describe("asistente", () => {
  it("paso 2: empareja con el código de 6 dígitos que muestran las dos máquinas", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { store } = await mountShell("demo&still&peer=none", "#/ajustes");
    await userEvent.click(screen.getByRole("button", { name: "Emparejar de nuevo" }));
    const dlg = screen.getByRole("dialog", { name: "Asistente" });
    expect(within(dlg).getByRole("heading", { name: "La otra máquina" })).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "Buscar PC-Escritorio" }));
    expect(await within(dlg).findByText("Buscando a PC-Escritorio…")).toBeInTheDocument();
    await vi.advanceTimersByTimeAsync(1500);
    const code = within(dlg).getByRole("group", { name: /mismo código/ });
    expect(code.textContent).toBe("418027");
    await userEvent.click(within(dlg).getByRole("button", { name: "Es el mismo" }));
    expect(await within(dlg).findByText("Esperando a que confirme PC-Escritorio…")).toBeInTheDocument();
    await vi.advanceTimersByTimeAsync(1300);
    expect(store.desk.pair.peer).toMatchObject({ kind: "paired", name: "PC-Escritorio" });
    expect(await within(dlg).findByText(/Emparejada/)).toBeInTheDocument();
    // Seguir deja al agente en reposo y pasa a Permisos.
    await userEvent.click(within(dlg).getByRole("button", { name: "Continuar" }));
    expect(store.desk.pair.pairing.kind).toBe("idle");
    expect(within(dlg).getByRole("heading", { name: "Permisos de macOS" })).toBeInTheDocument();
    vi.useRealTimers();
  });

  it("paso 2: «No coincide» cancela, y desemparejar pide confirmación", async () => {
    const { store } = await mountShell("demo&still&pair=code");
    store.openWizard(1);
    await tick();
    const dlg = screen.getByRole("dialog", { name: "Asistente" });
    await userEvent.click(within(dlg).getByRole("button", { name: "No coincide" }));
    expect(store.desk.pair.pairing.kind).toBe("idle");
    await userEvent.click(within(dlg).getByRole("button", { name: "Cambiar la otra máquina" }));
    await userEvent.click(within(dlg).getByRole("button", { name: "Desemparejar" }));
    expect(store.desk.pair.peer.kind).toBe("unpaired");
    expect(within(dlg).getByRole("button", { name: "Buscar PC-Escritorio" })).toBeInTheDocument();
  });

  it("paso 3: los permisos que faltan se abren en Ajustes del Sistema y se ven concedidos", async () => {
    const { store } = await mountShell("demo&still&trouble=permissions");
    await userEvent.click(screen.getByRole("button", { name: "Revisar permisos" }));
    const dlg = screen.getByRole("dialog", { name: "Asistente" });
    expect(within(dlg).getByRole("heading", { name: "Permisos de macOS" })).toBeInTheDocument();
    expect(within(dlg).getByText("Faltan 2")).toBeInTheDocument();
    expect(within(dlg).getByText(/tandemd \(pid 4242\)/)).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "Pedir permisos" }));
    const open = within(dlg).getAllByRole("button", { name: "Abrir ajustes" });
    expect(open).toHaveLength(2);
    await userEvent.click(open[0]!);
    await userEvent.click(within(dlg).getByRole("button", { name: "Abrir ajustes" }));
    expect(store.desk.permissions.rows.filter((r) => r.state === "missing")).toEqual([]);
    expect(within(dlg).getAllByText("Concedido")).toHaveLength(2);
    expect(within(dlg).queryByRole("button", { name: "Pedir permisos" })).toBeNull();
  });

  it("paso 3 en Windows: no hace falta ningún permiso", async () => {
    const { store } = await mountShell("demo&still&here=b");
    store.openWizard(2);
    await tick();
    expect(screen.getByText("Esta máquina no necesita permisos")).toBeInTheDocument();
  });

  it("los pasos sin contenido todavía no se pueden abrir y «Continuar» los salta", async () => {
    const { store } = await mountShell();
    store.openWizard(2);
    await tick();
    const dlg = screen.getByRole("dialog", { name: "Asistente" });
    expect(within(dlg).getByRole("button", { name: /Teclado y mouse/ })).toBeDisabled();
    await userEvent.click(within(dlg).getByRole("button", { name: "Continuar" }));
    expect(store.wizardStep).toBe(5);
    await userEvent.click(within(dlg).getByRole("button", { name: "Terminar" }));
    expect(store.wizardOpen).toBe(false);
  });
});

describe("Ajustes → Atajos", () => {
  it("graba una combinación, avisa si no sirve y la guarda", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell("demo&still", "#/ajustes");
    await user.click(screen.getByRole("tab", { name: "Atajos" }));
    const combo = await screen.findByRole("button", { name: /Atajo de «Mostrar u ocultar Tandem»/ });
    expect(combo).toHaveTextContent("Ctrl+Alt+0");
    await user.click(combo);
    expect(combo).toHaveTextContent("Pulsa…");
    await fireEvent.keyDown(combo, { key: "j", code: "KeyJ" });
    expect(screen.getByRole("alert")).toHaveTextContent("hace falta Ctrl");
    await fireEvent.keyDown(combo, { key: "j", code: "KeyJ", ctrlKey: true, shiftKey: true });
    expect(combo).toHaveTextContent("Ctrl+Mayús+J");
    await user.click(screen.getByRole("button", { name: "Guardar" }));
    await vi.waitFor(async () => expect((await store.shortcuts()).bindings.toggle_window).toBe("Ctrl+Shift+KeyJ"));
    expect(store.toasts.at(-1)?.text).toBe("Atajos guardados");
    await user.click(screen.getByRole("button", { name: "Restablecer" }));
    expect(combo).toHaveTextContent("Ctrl+Alt+0");
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
