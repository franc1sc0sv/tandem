// Ajustes (F-Ajustes, T-120): renombrar, interruptores de teclado y mouse, archivos y logs,
// copiar el comando, salir del puesto y las pestañas con el teclado.
import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { STORE_KEY } from "../lib/context";
import { nextTab, SETTINGS_TABS } from "../lib/settings";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import Ajustes from "./Ajustes.svelte";

afterEach(() => {
  vi.restoreAllMocks();
});

async function mount(query = "demo&still") {
  const store = new Store(new DemoSource(demoOptions(query)));
  await store.start();
  store.go({ screen: "ajustes" });
  render(Ajustes, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const tab = (name: RegExp) => screen.getByRole("tab", { name });

describe("pestañas", () => {
  it("hay cinco y se cambia con clic", async () => {
    await mount();
    expect(screen.getAllByRole("tab")).toHaveLength(5);
    expect(tab(/Puesto y máquinas/)).toHaveAttribute("aria-selected", "true");
    await userEvent.click(tab(/Teclado y mouse/));
    expect(tab(/Teclado y mouse/)).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("switch", { name: "Portapapeles compartido" })).toBeInTheDocument();
  });

  it("las flechas, Inicio y Fin las recorren y llevan el foco", async () => {
    await mount();
    tab(/Puesto y máquinas/).focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(tab(/Atajos/)).toHaveAttribute("aria-selected", "true");
    expect(tab(/Atajos/)).toHaveFocus();
    await userEvent.keyboard("{End}");
    expect(tab(/Avanzado/)).toHaveAttribute("aria-selected", "true");
    await userEvent.keyboard("{ArrowUp}");
    expect(tab(/Actualizaciones/)).toHaveAttribute("aria-selected", "true");
    await userEvent.keyboard("{Home}");
    expect(tab(/Puesto y máquinas/)).toHaveAttribute("aria-selected", "true");
  });

  it("nextTab no da la vuelta y ignora otras teclas", () => {
    expect(nextTab("puesto", "ArrowUp")).toBeNull();
    expect(nextTab("avanzado", "ArrowDown")).toBeNull();
    expect(nextTab("puesto", "a")).toBeNull();
    expect(nextTab("puesto", "End")).toBe(SETTINGS_TABS[SETTINGS_TABS.length - 1].id);
    expect(nextTab("atajos", "Home")).toBe("puesto");
  });
});

describe("Puesto y máquinas", () => {
  it("el nombre del puesto es texto, no un campo que no hace nada", async () => {
    await mount();
    expect(screen.getByTestId("desk-name")).toHaveTextContent("Mi puesto");
    expect(screen.queryByRole("textbox", { name: "Nombre del puesto" })).toBeNull();
  });

  it("renombrar una máquina: Enter manda el nombre nuevo al puesto", async () => {
    const store = await mount();
    const edit = vi.spyOn(store, "editMachine");
    const name = store.desk.machines.b?.name ?? "";
    await userEvent.click(screen.getAllByRole("button", { name: "Renombrar" })[1]);
    const input = screen.getByRole("textbox", { name: `Nombre de ${name}` });
    await userEvent.clear(input);
    await userEvent.type(input, "Taller{Enter}");
    expect(edit).toHaveBeenCalledWith("b", { name: "Taller" });
  });

  it("Escape cancela sin cambiar nada; un nombre vacío o igual tampoco cambia", async () => {
    const store = await mount();
    const edit = vi.spyOn(store, "editMachine");
    const first = store.desk.machines.a?.name ?? "";
    await userEvent.click(screen.getAllByRole("button", { name: "Renombrar" })[0]);
    await userEvent.type(screen.getByRole("textbox", { name: `Nombre de ${first}` }), "Otro{Escape}");
    expect(screen.queryByRole("textbox", { name: /Nombre de/ })).toBeNull();
    await userEvent.click(screen.getAllByRole("button", { name: "Renombrar" })[0]);
    const input = screen.getByRole("textbox", { name: `Nombre de ${first}` });
    await userEvent.clear(input);
    await userEvent.type(input, "   {Enter}");
    await userEvent.click(screen.getAllByRole("button", { name: "Renombrar" })[0]);
    await userEvent.type(screen.getByRole("textbox", { name: `Nombre de ${first}` }), "{Enter}");
    expect(edit).not.toHaveBeenCalled();
  });

  it("«Emparejar de nuevo» y «Abrir» abren el asistente", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openWizard");
    await userEvent.click(screen.getByRole("button", { name: /Emparejar de nuevo/ }));
    expect(open).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Abrir" }));
    expect(open).toHaveBeenLastCalledWith(0);
  });

  it("«Salir» pide confirmación y solo entonces desempareja", async () => {
    const store = await mount();
    const pair = vi.spyOn(store, "pairAction");
    await userEvent.click(screen.getByRole("button", { name: "Salir" }));
    expect(pair).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Confirmar" }));
    expect(pair).toHaveBeenCalledWith("unpair");
    expect(screen.getByRole("button", { name: "Salir" })).toBeInTheDocument();
  });
});

describe("Teclado y mouse", () => {
  it("«Cmd por Ctrl» y «Arrastrar» mandan el par entero de preferencias", async () => {
    const store = await mount();
    const set = vi.spyOn(store, "setPrefs");
    await userEvent.click(tab(/Teclado y mouse/));
    const was = store.desk.prefs;
    await userEvent.click(screen.getByRole("switch", { name: "Cambiar Cmd por Ctrl" }));
    expect(set).toHaveBeenLastCalledWith({ cmdCtrl: !was.cmdCtrl, dragFiles: was.dragFiles });
    await userEvent.click(screen.getByRole("switch", { name: "Arrastrar entre máquinas" }));
    expect(set).toHaveBeenLastCalledWith({ cmdCtrl: !was.cmdCtrl, dragFiles: !was.dragFiles });
    expect(screen.getByRole("switch", { name: "Cambiar Cmd por Ctrl" })).toHaveAttribute("aria-checked", String(!was.cmdCtrl));
  });

  it("el portapapeles se enciende o apaga en las dos máquinas", async () => {
    const store = await mount();
    const mod = vi.spyOn(store, "setModule");
    await userEvent.click(tab(/Teclado y mouse/));
    const was = store.desk.prefs.clipboard;
    await userEvent.click(screen.getByRole("switch", { name: "Portapapeles compartido" }));
    await vi.waitFor(() => expect(mod).toHaveBeenCalledTimes(2));
    expect(mod.mock.calls.map((c) => [c[0], c[1], c[2]]).sort()).toEqual([
      ["a", "clipboard", !was],
      ["b", "clipboard", !was],
    ]);
  });
});

describe("Avanzado", () => {
  it("muestra la ruta del archivo de configuración y abre archivo y logs", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openFiles");
    await userEvent.click(tab(/Avanzado/));
    await vi.waitFor(() => expect(screen.getByText(/tandem\.toml/)).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "Mostrar archivo" }));
    expect(open).toHaveBeenLastCalledWith("config");
    await userEvent.click(screen.getByRole("button", { name: "Abrir carpeta" }));
    expect(open).toHaveBeenLastCalledWith("logs");
  });

  it("copia el comando del servidor MCP y avisa", async () => {
    const store = await mount();
    const write = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
    await userEvent.click(tab(/Avanzado/));
    await userEvent.click(screen.getByRole("button", { name: "Copiar comando" }));
    expect(write).toHaveBeenCalledWith("tandem mcp");
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Comando copiado", tone: "ok" }));
  });

  it("si no se puede copiar, lo dice", async () => {
    const store = await mount();
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: async () => {
          throw new Error("sin permiso");
        },
      },
      configurable: true,
    });
    await userEvent.click(tab(/Avanzado/));
    await userEvent.click(screen.getByRole("button", { name: "Copiar comando" }));
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "No se pudo copiar el comando", tone: "bad" }));
  });

  it("«Ver» lleva a los estados difíciles", async () => {
    const store = await mount();
    await userEvent.click(tab(/Avanzado/));
    const row = screen.getByText(/Estados difíciles/).closest(".row") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: "Ver" }));
    expect(store.route).toEqual({ screen: "estados" });
  });
});
