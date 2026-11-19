import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import { STORE_KEY } from "../lib/context";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import Tray from "../Tray.svelte";
import Shell from "./Shell.svelte";

async function mountShell(query = "demo&still", hash = "#/desk") {
  const loc = { hash };
  const store = new Store(new DemoSource(demoOptions(query)), loc);
  await store.start();
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return { ...r, store, loc };
}

describe("sidebar (D16)", () => {
  it("has the screens, the tunnel counter, one entry per machine with its color and Settings", async () => {
    await mountShell();
    const nav = screen.getByRole("navigation", { name: "Main" });
    const links = within(nav).getAllByRole("link").map((a) => a.textContent?.replace(/\s+/g, " ").trim());
    expect(links).toEqual(["Desk", "Tunnels 3", "Monitors", "Arrangement", "Network", "MacBook Pro", "PC-Desktop", "Settings"]);
    expect(within(nav).getByText("Machines")).toBeInTheDocument();
    const dot = within(nav).getByRole("link", { name: "PC-Desktop" }).querySelector(".mdot") as HTMLElement;
    expect(dot.style.background).toBe("rgb(169, 155, 232)");
    // D1: no third machine.
    expect(within(nav).queryByText(/Add/)).toBeNull();
  });

  it("marks the current screen and navigates with a click", async () => {
    const { store, loc } = await mountShell();
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: "Desk" })).toHaveAttribute("aria-current", "page");
    await userEvent.click(within(nav).getByRole("link", { name: "Network" }));
    expect(store.route).toEqual({ screen: "network" });
    expect(loc.hash).toBe("#/network");
    expect(within(nav).getByRole("link", { name: "Network" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Network");
    await userEvent.click(within(nav).getByRole("link", { name: "MacBook Pro" }));
    expect(store.route).toEqual({ screen: "machine", machine: "a" });
  });

  it("with two Windows, the names and colors come from each machine (D2)", async () => {
    await mountShell("demo=win-win&still");
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: "PC-Desktop" })).toBeInTheDocument();
    expect(within(nav).getByRole("link", { name: "PC-Workshop" })).toBeInTheDocument();
    expect(within(nav).queryByText("MacBook Pro")).toBeNull();
  });
});

describe("palette ⌘K / Ctrl+K (D15)", () => {
  it("Ctrl+K opens it with focus on the search; Escape closes it", async () => {
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

  it("the \"Search or run…\" button also opens it", async () => {
    const { store } = await mountShell();
    await userEvent.click(screen.getByRole("button", { name: /Search or run/ }));
    expect(store.paletteOpen).toBe(true);
  });

  it("filters while typing, moves with arrows and runs with Enter", async () => {
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

  it("without results it says so", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    await user.type(screen.getByRole("combobox", { name: "Search" }), "zzz");
    expect(screen.queryAllByRole("option")).toEqual([]);
    expect(screen.getByText("Nothing matches “zzz”.")).toBeInTheDocument();
  });

  it("going to a screen from the palette, with a click", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell();
    store.togglePalette(true);
    await tick();
    await user.type(screen.getByRole("combobox", { name: "Search" }), "arrangement");
    await user.click(screen.getByRole("option", { name: /Arrangement/ }));
    expect(store.route).toEqual({ screen: "arrangement" });
    expect(store.paletteOpen).toBe(false);
  });

  it("opening the setup from the palette", async () => {
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

describe("screens", () => {
  it("Desk shows the two machines and applies scenes", async () => {
    const { store } = await mountShell();
    expect(screen.getByRole("link", { name: /MacBook Pro.*macOS · M5/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Game/ }));
    await vi.waitFor(() => expect(store.desk.activeScene).toBe("game"));
    expect(screen.getByRole("button", { name: /Game/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("a trouble state replaces the Desk and offers the action", async () => {
    const { store } = await mountShell("demo&still&trouble=peer-off");
    expect(screen.getByText("PC-Desktop is off")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Power on PC-Desktop" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("on"));
  });

  it("a machine's modules are turned off with its switch", async () => {
    const { store } = await mountShell("demo&still", "#/machine/b");
    await userEvent.click(screen.getByRole("switch", { name: "Clipboard" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("off"));
  });

  it("⌘, / Ctrl+, va a Settings", async () => {
    const { store } = await mountShell();
    await fireEvent.keyDown(window, { key: ",", ctrlKey: true });
    expect(store.route).toEqual({ screen: "settings" });
  });
});

describe("Settings → Shortcuts", () => {
  it("records a combination, warns if it is no good and saves it", async () => {
    const user = userEvent.setup();
    const { store } = await mountShell("demo&still", "#/settings");
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

describe("tray (V3-Tray)", () => {
  it("shows the machines, applies scenes, opens the window and quits", async () => {
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
