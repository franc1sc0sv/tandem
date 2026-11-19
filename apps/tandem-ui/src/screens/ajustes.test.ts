// Ajustes (F-Settings, T-120): renombrar, interruptores de teclado y mouse, archivos y logs,
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
    expect(tab(/Desk and machines/)).toHaveAttribute("aria-selected", "true");
    await userEvent.click(tab(/Keyboard and mouse/));
    expect(tab(/Keyboard and mouse/)).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("switch", { name: "Shared clipboard" })).toBeInTheDocument();
  });

  it("las flechas, Inicio y Fin las recorren y llevan el foco", async () => {
    await mount();
    tab(/Desk and machines/).focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(tab(/Shortcuts/)).toHaveAttribute("aria-selected", "true");
    expect(tab(/Shortcuts/)).toHaveFocus();
    await userEvent.keyboard("{End}");
    expect(tab(/Advanced/)).toHaveAttribute("aria-selected", "true");
    await userEvent.keyboard("{ArrowUp}");
    expect(tab(/Updates/)).toHaveAttribute("aria-selected", "true");
    await userEvent.keyboard("{Home}");
    expect(tab(/Desk and machines/)).toHaveAttribute("aria-selected", "true");
  });

  it("nextTab no da la vuelta y ignora otras teclas", () => {
    expect(nextTab("puesto", "ArrowUp")).toBeNull();
    expect(nextTab("avanzado", "ArrowDown")).toBeNull();
    expect(nextTab("puesto", "a")).toBeNull();
    expect(nextTab("puesto", "End")).toBe(SETTINGS_TABS[SETTINGS_TABS.length - 1].id);
    expect(nextTab("atajos", "Home")).toBe("puesto");
  });
});

describe("Desk and machines", () => {
  it("el nombre del puesto es texto, no un campo que no hace nada", async () => {
    await mount();
    expect(screen.getByTestId("desk-name")).toHaveTextContent("My desk");
    expect(screen.queryByRole("textbox", { name: "Desk name" })).toBeNull();
  });

  it("renombrar una máquina: Enter manda el nombre nuevo al puesto", async () => {
    const store = await mount();
    const edit = vi.spyOn(store, "editMachine");
    const name = store.desk.machines.b?.name ?? "";
    await userEvent.click(screen.getAllByRole("button", { name: "Rename" })[1]);
    const input = screen.getByRole("textbox", { name: `Name of ${name}` });
    await userEvent.clear(input);
    await userEvent.type(input, "Taller{Enter}");
    expect(edit).toHaveBeenCalledWith("b", { name: "Taller" });
  });

  it("Escape cancela sin cambiar nada; un nombre vacío o igual tampoco cambia", async () => {
    const store = await mount();
    const edit = vi.spyOn(store, "editMachine");
    const first = store.desk.machines.a?.name ?? "";
    await userEvent.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    await userEvent.type(screen.getByRole("textbox", { name: `Name of ${first}` }), "Otro{Escape}");
    expect(screen.queryByRole("textbox", { name: /Name de/ })).toBeNull();
    await userEvent.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    const input = screen.getByRole("textbox", { name: `Name of ${first}` });
    await userEvent.clear(input);
    await userEvent.type(input, "   {Enter}");
    await userEvent.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    await userEvent.type(screen.getByRole("textbox", { name: `Name of ${first}` }), "{Enter}");
    expect(edit).not.toHaveBeenCalled();
  });

  it("«Pair again» y «Abrir» abren el asistente", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openWizard");
    await userEvent.click(screen.getByRole("button", { name: /Pair again/ }));
    expect(open).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(open).toHaveBeenLastCalledWith(0);
  });

  it("«Salir» dice con exactitud cuándo the other machine olvida la llave (T-152)", async () => {
    await mount();
    expect(screen.getByText(/if they are connected, they forget its key; if not, they do once they leave too/)).toBeInTheDocument();
  });

  it("«Salir» pide confirmación y solo entonces desempareja", async () => {
    const store = await mount();
    const pair = vi.spyOn(store, "pairAction");
    await userEvent.click(screen.getByRole("button", { name: "Leave" }));
    expect(pair).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(pair).toHaveBeenCalledWith("unpair");
    expect(screen.getByRole("button", { name: "Leave" })).toBeInTheDocument();
  });
});

describe("Keyboard and mouse", () => {
  it("«Cmd por Ctrl» y «Arrastrar» mandan el par entero de preferencias", async () => {
    const store = await mount();
    const set = vi.spyOn(store, "setPrefs");
    await userEvent.click(tab(/Keyboard and mouse/));
    const was = store.desk.prefs;
    await userEvent.click(screen.getByRole("switch", { name: "Swap Cmd and Ctrl" }));
    expect(set).toHaveBeenLastCalledWith({ cmdCtrl: !was.cmdCtrl, dragFiles: was.dragFiles });
    await userEvent.click(screen.getByRole("switch", { name: "Drag between machines" }));
    expect(set).toHaveBeenLastCalledWith({ cmdCtrl: !was.cmdCtrl, dragFiles: !was.dragFiles });
    expect(screen.getByRole("switch", { name: "Swap Cmd and Ctrl" })).toHaveAttribute("aria-checked", String(!was.cmdCtrl));
  });

  it("el portapapeles se enciende o apaga en las dos máquinas", async () => {
    const store = await mount();
    const mod = vi.spyOn(store, "setModule");
    await userEvent.click(tab(/Keyboard and mouse/));
    const was = store.desk.prefs.clipboard;
    await userEvent.click(screen.getByRole("switch", { name: "Shared clipboard" }));
    await vi.waitFor(() => expect(mod).toHaveBeenCalledTimes(2));
    expect(mod.mock.calls.map((c) => [c[0], c[1], c[2]]).sort()).toEqual([
      ["a", "clipboard", !was],
      ["b", "clipboard", !was],
    ]);
  });
});

describe("Advanced", () => {
  it("muestra la ruta del archivo de configuración y abre archivo y logs", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openFiles");
    await userEvent.click(tab(/Advanced/));
    await vi.waitFor(() => expect(screen.getByText(/tandem\.toml/)).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "Show file" }));
    expect(open).toHaveBeenLastCalledWith("config");
    await userEvent.click(screen.getByRole("button", { name: "Open folder" }));
    expect(open).toHaveBeenLastCalledWith("logs");
  });

  it("copia el comando del servidor MCP y avisa", async () => {
    const store = await mount();
    const write = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
    await userEvent.click(tab(/Advanced/));
    await userEvent.click(screen.getByRole("button", { name: "Copy command" }));
    expect(write).toHaveBeenCalledWith("tandem mcp");
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Command copied", tone: "ok" }));
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
    await userEvent.click(tab(/Advanced/));
    await userEvent.click(screen.getByRole("button", { name: "Copy command" }));
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Could not copy the command", tone: "bad" }));
  });

  it("«Ver» lleva a los estados difíciles", async () => {
    const store = await mount();
    await userEvent.click(tab(/Advanced/));
    const row = screen.getByText(/Trouble states/).closest(".row") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: "View" }));
    expect(store.route).toEqual({ screen: "estados" });
  });
});
