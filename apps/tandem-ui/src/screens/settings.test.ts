// Settings (F-Settings, T-120): renaming, keyboard and mouse switches, files and logs,
// copying the command, leaving the desk and the tabs with the keyboard.
import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { STORE_KEY } from "../lib/context";
import { nextTab, SETTINGS_TABS } from "../lib/settings";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import Settings from "./Settings.svelte";

afterEach(() => {
  vi.restoreAllMocks();
});

async function mount(query = "demo&still") {
  const store = new Store(new DemoSource(demoOptions(query)));
  await store.start();
  store.go({ screen: "settings" });
  render(Settings, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const tab = (name: RegExp) => screen.getByRole("tab", { name });

describe("tabs", () => {
  it("there are five and it changes with a click", async () => {
    await mount();
    expect(screen.getAllByRole("tab")).toHaveLength(5);
    expect(tab(/Desk and machines/)).toHaveAttribute("aria-selected", "true");
    await userEvent.click(tab(/Keyboard and mouse/));
    expect(tab(/Keyboard and mouse/)).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("switch", { name: "Shared clipboard" })).toBeInTheDocument();
  });

  it("the arrows, Home and End go through them and carry the focus", async () => {
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

  it("nextTab does not wrap around and ignores other keys", () => {
    expect(nextTab("desk", "ArrowUp")).toBeNull();
    expect(nextTab("advanced", "ArrowDown")).toBeNull();
    expect(nextTab("desk", "a")).toBeNull();
    expect(nextTab("desk", "End")).toBe(SETTINGS_TABS[SETTINGS_TABS.length - 1].id);
    expect(nextTab("shortcuts", "Home")).toBe("desk");
  });
});

describe("Desk and machines", () => {
  it("the desk name is text, not a field that does nothing", async () => {
    await mount();
    expect(screen.getByTestId("desk-name")).toHaveTextContent("My desk");
    expect(screen.queryByRole("textbox", { name: "Desk name" })).toBeNull();
  });

  it("renaming a machine: Enter sends the new name to the desk", async () => {
    const store = await mount();
    const edit = vi.spyOn(store, "editMachine");
    const name = store.desk.machines.b?.name ?? "";
    await userEvent.click(screen.getAllByRole("button", { name: "Rename" })[1]);
    const input = screen.getByRole("textbox", { name: `Name of ${name}` });
    await userEvent.clear(input);
    await userEvent.type(input, "Workshop{Enter}");
    expect(edit).toHaveBeenCalledWith("b", { name: "Workshop" });
  });

  it("Escape cancels without changing anything; an empty or identical name does not change either", async () => {
    const store = await mount();
    const edit = vi.spyOn(store, "editMachine");
    const first = store.desk.machines.a?.name ?? "";
    await userEvent.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    await userEvent.type(screen.getByRole("textbox", { name: `Name of ${first}` }), "Other{Escape}");
    expect(screen.queryByRole("textbox", { name: /Name de/ })).toBeNull();
    await userEvent.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    const input = screen.getByRole("textbox", { name: `Name of ${first}` });
    await userEvent.clear(input);
    await userEvent.type(input, "   {Enter}");
    await userEvent.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    await userEvent.type(screen.getByRole("textbox", { name: `Name of ${first}` }), "{Enter}");
    expect(edit).not.toHaveBeenCalled();
  });

  it("\"Pair again\" and \"Open\" open the setup", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openWizard");
    await userEvent.click(screen.getByRole("button", { name: /Pair again/ }));
    expect(open).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(open).toHaveBeenLastCalledWith(0);
  });

  it("\"Leave\" says exactly when the other machine forgets the key (T-152)", async () => {
    await mount();
    expect(screen.getByText(/if they are connected, they forget its key; if not, they do once they leave too/)).toBeInTheDocument();
  });

  it("\"Leave\" asks for confirmation and only then unpairs", async () => {
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
  it("\"Cmd for Ctrl\" and \"Drag\" send the whole pair of preferences", async () => {
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

  it("the clipboard is turned on or off on both machines", async () => {
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
  it("shows the path of the configuration file and opens the file and logs", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openFiles");
    await userEvent.click(tab(/Advanced/));
    await vi.waitFor(() => expect(screen.getByText(/tandem\.toml/)).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "Show file" }));
    expect(open).toHaveBeenLastCalledWith("config");
    await userEvent.click(screen.getByRole("button", { name: "Open folder" }));
    expect(open).toHaveBeenLastCalledWith("logs");
  });

  it("copies the MCP server command and announces it", async () => {
    const store = await mount();
    const write = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
    await userEvent.click(tab(/Advanced/));
    await userEvent.click(screen.getByRole("button", { name: "Copy command" }));
    expect(write).toHaveBeenCalledWith("tandem mcp");
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Command copied", tone: "ok" }));
  });

  it("if it cannot be copied, it says so", async () => {
    const store = await mount();
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: async () => {
          throw new Error("no permission");
        },
      },
      configurable: true,
    });
    await userEvent.click(tab(/Advanced/));
    await userEvent.click(screen.getByRole("button", { name: "Copy command" }));
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ text: "Could not copy the command", tone: "bad" }));
  });

  it("\"View\" leads to the trouble states", async () => {
    const store = await mount();
    await userEvent.click(tab(/Advanced/));
    const row = screen.getByText(/Trouble states/).closest(".row") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: "View" }));
    expect(store.route).toEqual({ screen: "states" });
  });
});
