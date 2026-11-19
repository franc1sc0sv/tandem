// Machine page (T-115): what it shows and what can be done, in demo mode and against a
// source that talks to the agent (simulated IPC).
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
import Machine from "./Machine.svelte";

afterEach(() => {
  vi.useRealTimers();
});

async function mount(query = "demo&still", machine: "a" | "b" = "b") {
  const store = new Store(new DemoSource(demoOptions(query)));
  await store.start();
  store.go({ screen: "machine", machine });
  const r = render(Machine, { props: { machine }, context: new Map([[STORE_KEY, store]]) });
  await tick();
  return { ...r, store };
}

const res = () => screen.getByRole("region", { name: "Live resources" });

describe("Wake-on-LAN (T-159)", () => {
  it("the field is always there, with the current MAC and a button to delete it", async () => {
    const { store } = await mount();
    const set = vi.spyOn(store, "setWol");
    const field = screen.getByRole("textbox", { name: /MAC of/ });
    expect(field).toHaveValue("aa:bb:cc:dd:ee:ff");
    await userEvent.click(screen.getByRole("button", { name: "Clear MAC" }));
    expect(set).toHaveBeenCalledWith(null);
  });

  it("editing the MAC saves it", async () => {
    const { store } = await mount();
    const set = vi.spyOn(store, "setWol");
    const field = screen.getByRole("textbox", { name: /MAC of/ });
    await userEvent.clear(field);
    await userEvent.type(field, "11:22:33:44:55:66");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(set).toHaveBeenCalledWith("11:22:33:44:55:66");
  });
});

describe("Wake-on-LAN en el demo (T-171)", () => {
  const mac = () => screen.getByRole("textbox", { name: /MAC of/ });
  const save = () => screen.getByRole("button", { name: "Save" });

  it("saving a valid MAC shows it and leaves Save disabled", async () => {
    const { store } = await mount();
    await userEvent.click(screen.getByRole("button", { name: "Power off" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("off"));
    await userEvent.clear(mac());
    await userEvent.type(mac(), "aa:bb:cc:dd:ee:01");
    await userEvent.click(save());
    await tick();
    expect(mac()).toHaveValue("aa:bb:cc:dd:ee:01");
    expect(save()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Power on" })).toBeEnabled();
  });

  it("deleting the MAC leaves the field empty, without Power on and with the notice", async () => {
    const { store } = await mount();
    await userEvent.click(screen.getByRole("button", { name: "Power off" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("off"));
    await userEvent.click(screen.getByRole("button", { name: "Clear MAC" }));
    await tick();
    expect(mac()).toHaveValue("");
    expect(screen.getByRole("button", { name: "Power on" })).toBeDisabled();
    expect(screen.getByText(/MAC is missing/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Clear MAC" })).toBeNull();
  });

  it("an invalid MAC shows the reason and does not change what is saved", async () => {
    const { store } = await mount();
    await userEvent.clear(mac());
    await userEvent.type(mac(), "zz");
    await userEvent.click(save());
    await tick();
    expect(store.toasts.at(-1)?.text ?? "").toMatch(/invalid MAC address/);
    expect(store.desk.machines.b?.wake?.mac).toBe("aa:bb:cc:dd:ee:ff");
  });
});

describe("V3-Machine (the other machine)", () => {
  it("shows header, grouped resources, disk, modules and environments", async () => {
    await mount();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("PC-Desktop");
    expect(screen.getByText("Windows 11 · online")).toBeInTheDocument();
    const r = within(res());
    expect(r.getByText("Ryzen 7 · 16 threads")).toBeInTheDocument();
    expect(r.getByText("64 %")).toBeInTheDocument();
    expect(r.getByText("62 °C")).toBeInTheDocument();
    expect(r.getByText("17.1")).toBeInTheDocument();
    expect(r.getByText("/ 24 GB")).toBeInTheDocument();
    expect(r.getByText("RTX 5080")).toBeInTheDocument();
    expect(r.getByText("VRAM almost full")).toBeInTheDocument();
    expect(r.getByText("97 %")).toBeInTheDocument();
    expect(r.getByText("71 °C")).toBeInTheDocument();
    expect(r.getByText("fans 58 %")).toBeInTheDocument();
    expect(screen.getByText("412 GB free of 1.8 TB")).toBeInTheDocument();
    const mods = within(screen.getByRole("region", { name: "Modules" }));
    expect(mods.getAllByText("active")).toHaveLength(6);
    const envs = within(screen.getByRole("region", { name: "Environments" }));
    // The default (WSL) goes first even if the agent says it second.
    const names = envs.getAllByText(/WSL · Ubuntu|PowerShell 7|Docker Desktop/).map((e) => e.textContent);
    expect(names).toEqual(["WSL · Ubuntu 24.04", "PowerShell 7", "Docker Desktop"]);
    expect(envs.getByText("default")).toBeInTheDocument();
    expect(envs.getByRole("switch", { name: "Allow remote execution" })).toHaveAttribute("aria-checked", "true");
    // G9: no broken texts.
    expect(document.body.textContent).not.toMatch(/undefined|NaN|\{\{/);
  });

  it("the color is changed in the desk; the other machine's cannot be chosen", async () => {
    const { store } = await mount();
    const group = screen.getByRole("radiogroup", { name: "Machine color" });
    expect(within(group).getByRole("radio", { name: "Violet" })).toHaveAttribute("aria-checked", "true");
    expect(within(group).getByRole("radio", { name: "Blue" })).toBeDisabled();
    await userEvent.click(within(group).getByRole("radio", { name: "Teal" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.color).toBe("#5fbfb0"));
    expect(within(group).getByRole("radio", { name: "Teal" })).toHaveAttribute("aria-checked", "true");
    // The arrows skip the taken color (blue, the MacBook's).
    await fireEvent.keyDown(group, { key: "ArrowRight" });
    await vi.waitFor(() => expect(store.desk.machines.b?.color).toBe("#d9a75a"));
    await fireEvent.keyDown(group, { key: "ArrowRight" });
    await vi.waitFor(() => expect(store.desk.machines.b?.color).toBe("#a99be8"));
  });

  it("the name is edited in place: Enter saves, Escape does not", async () => {
    const { store } = await mount();
    await userEvent.click(screen.getByRole("button", { name: /PC-Desktop/ }));
    const input = screen.getByRole("textbox", { name: "Machine name" });
    await userEvent.clear(input);
    await userEvent.type(input, "  Studio {Enter}");
    await vi.waitFor(() => expect(store.desk.machines.b?.name).toBe("Studio"));
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Studio");
    await userEvent.click(screen.getByRole("button", { name: /Studio/ }));
    await userEvent.type(screen.getByRole("textbox", { name: "Machine name" }), "XYZ{Escape}");
    expect(store.desk.machines.b?.name).toBe("Studio");
    // Empty: it is not sent and the reason is announced.
    await userEvent.click(screen.getByRole("button", { name: /Studio/ }));
    await userEvent.clear(screen.getByRole("textbox", { name: "Machine name" }));
    await userEvent.keyboard("{Enter}");
    expect(store.desk.machines.b?.name).toBe("Studio");
    expect(store.toasts[0]).toMatchObject({ tone: "bad", text: "The name cannot be empty" });
  });

  it("each module that can be turned off has its switch; Power does not", async () => {
    const { store } = await mount();
    const mods = within(screen.getByRole("region", { name: "Modules" }));
    const clip = mods.getByRole("switch", { name: "Clipboard" });
    expect(clip).toHaveAttribute("aria-checked", "true");
    await userEvent.click(clip);
    await vi.waitFor(() => expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("off"));
    expect(mods.getByRole("switch", { name: "Clipboard" })).toHaveTextContent("off");
    await userEvent.click(mods.getByRole("switch", { name: "Clipboard" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.modules.find((m) => m.id === "clipboard")?.state).toBe("active"));
    expect(mods.queryByRole("switch", { name: "Power" })).toBeNull();
    expect(mods.getByText("Power").parentElement).toHaveTextContent("active");
  });

  it("\"Allow remote execution\" shows the tunnels module, which the agent cannot turn off (T-098, T-244)", async () => {
    const { store } = await mount();
    const sw = screen.getByRole("switch", { name: "Allow remote execution" });
    expect(sw).toHaveAttribute("aria-checked", "true");
    expect(sw).toBeDisabled();
    await userEvent.click(sw);
    expect(store.desk.machines.b?.modules.find((m) => m.id === "tunnels")?.state).toBe("active");
    expect(within(screen.getByRole("region", { name: "Modules" })).queryByRole("switch", { name: "Tunnels" })).toBeNull();
  });

  it("rescan shows that it works until it returns", async () => {
    vi.useFakeTimers();
    const { store } = await mount();
    const spy = vi.spyOn(store, "rescan");
    const btn = screen.getByRole("button", { name: "Scan again" });
    await fireEvent.click(btn);
    expect(spy).toHaveBeenCalledWith("b");
    expect(btn).toHaveAttribute("aria-busy", "true");
    await vi.advanceTimersByTimeAsync(1000);
    expect(btn).toHaveAttribute("aria-busy", "false");
  });

  it("Power off turns the other machine off and Power on remains", async () => {
    const { store } = await mount();
    await userEvent.click(screen.getByRole("button", { name: "Power off" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("off"));
    expect(screen.getByText("Windows 11 · off")).toBeInTheDocument();
    expect(screen.queryByText("64 %")).toBeNull();
    expect(screen.getByRole("button", { name: "Scan again" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Power on" }));
    await vi.waitFor(() => expect(store.desk.machines.b?.power).toBe("on"));
  });
});

describe("F-MachineMac (this machine)", () => {
  it("without Power off; unified RAM and integrated GPU with their memory", async () => {
    await mount("demo&still", "a");
    expect(screen.getByText("macOS 26 · Apple M5")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Power off" })).toBeNull();
    const r = within(res());
    expect(r.getByText("M5 · 12 cores")).toBeInTheDocument();
    expect(r.getByText("unified")).toBeInTheDocument();
    expect(r.getByText("M5 integrated")).toBeInTheDocument();
    expect(r.getByText("Memory")).toBeInTheDocument();
    expect(r.getByText("of RAM")).toBeInTheDocument();
    expect(r.queryByText("VRAM almost full")).toBeNull();
    expect(r.getByText("fanless")).toBeInTheDocument();
    expect(screen.getByText("231 GB free of 1 TB")).toBeInTheDocument();
    expect(screen.getByText("84 % · plugged in")).toBeInTheDocument();
    const envs = within(screen.getByRole("region", { name: "Environments" }));
    expect(envs.getByText("zsh")).toBeInTheDocument();
    expect(envs.getByText("default")).not.toHaveClass("chip");
    // This machine's tunnels module is turned off from its row: without the switch below.
    expect(envs.queryByRole("switch", { name: "Allow remote execution" })).toBeNull();
    // Remote compute has no switch: the agent cannot turn it off (T-098); it is a plain chip.
    expect(screen.queryByRole("switch", { name: "Remote compute" })).toBeNull();
    expect(within(screen.getByRole("region", { name: "Modules" })).getByText("Remote compute").parentElement).toHaveTextContent("active");
  });

  it("the desk name goes next to the logo only on the machine pages (V3-Machine)", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/desk" });
    await store.start();
    render(Shell, { context: new Map([[STORE_KEY, store]]) });
    await tick();
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).queryByText("My desk")).toBeNull();
    store.go({ screen: "machine", machine: "b" });
    await tick();
    expect(within(nav).getByText("My desk")).toBeInTheDocument();
  });

  it("if a permission is missing, the \"Permissions\" row says so and \"Open settings\" requests it", async () => {
    const { store } = await mount("demo&still&trouble=permissions", "a");
    const mods = within(screen.getByRole("region", { name: "Modules" }));
    const names = mods.getAllByText(/^(Keyboard and mouse|Permissions|Clipboard)$/).map((e) => e.textContent);
    expect(names).toEqual(["Keyboard and mouse", "Permissions", "Clipboard"]);
    expect(mods.getByText("“Input Monitoring” is missing: it cannot send the keyboard. It is checked automatically.")).toBeInTheDocument();
    const spy = vi.spyOn(store, "openPermission");
    await userEvent.click(mods.getByRole("button", { name: "Open settings" }));
    expect(spy).toHaveBeenCalledWith("input_monitoring");
    // With that one granted, the row moves to the next one missing; with everything granted, it disappears.
    await vi.waitFor(() => expect(mods.getByText(/“Accessibility” is missing/)).toBeInTheDocument());
    await userEvent.click(mods.getByRole("button", { name: "Open settings" }));
    await vi.waitFor(() => expect(mods.queryByText("Permissions")).toBeNull());
  });

  it("the other machine's page does not have the \"Permissions\" row", async () => {
    await mount("demo&still&trouble=permissions", "b");
    expect(screen.queryByText("Permissions")).toBeNull();
  });

  it("with two Macs, names, colors and icons come from each machine (D2)", async () => {
    await mount("demo=mac-mac&still", "b");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Mac mini");
    expect(screen.getByRole("radio", { name: "Amber" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Blue" })).toBeDisabled();
  });
});

describe("animations (G7, G8)", () => {
  const css = readFileSync(join(process.cwd(), "src", "screens", "Machine.svelte"), "utf8");

  it("the mockup ones are there: group and row entrance, lines, bars, notice and dot", () => {
    for (const k of ["up", "dr", "grow", "wp", "pl"]) expect(css).toMatch(new RegExp(`@keyframes ${k} \\{`));
    expect(css).toMatch(/\.grp \{[^}]*animation: up 0\.45s cubic-bezier\(0\.2, 0\.8, 0\.2, 1\) both/);
    expect(css).toMatch(/\.mod \{[^}]*animation: up 0\.4s ease-out both/);
    expect(css).toMatch(/\.ln \{[^}]*animation: dr 1\.6s ease-out 0\.2s forwards/);
    expect(css).toMatch(/\.warnchip \{[^}]*animation: wp 2\.4s ease-in-out infinite/);
  });

  it("with prefers-reduced-motion the lines stay drawn (the global rule turns off the rest)", () => {
    expect(css).toMatch(/@media \(prefers-reduced-motion: reduce\) \{\s*\.ln \{\s*stroke-dashoffset: 0;/);
  });
});

describe("against the agent (IPC)", () => {
  const GIB = 2 ** 30;
  const caps = {
    machine: "b",
    name: "Workshop",
    os: "windows",
    color: "#a99be8",
    hub: null,
    input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
    power: { wake_peer: false, wakeable: true, power_off: true },
    services: [],
    modules: [
      { id: "clipboard", name: "Clipboard", detail: "Text", state: "active", reason: null, switchable: true },
      { id: "secure_desktop", name: "Protected screens", detail: "UAC", state: "unavailable", reason: "The administrator service is missing", switchable: false },
      { id: "tunnels", name: "Tunnels", detail: "", state: "active", reason: null, switchable: true },
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
    desk: { machines: { a: null, b: null }, screens: [], arrangement: null, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "Workshop", link: "usb4" },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
    caps: { here: { ...caps, machine: "a", name: "Studio", os: "macos", color: "#7aa7e6", modules: [], environments: [], hardware: undefined }, peer: caps },
    monitors: [],
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
    store.go({ screen: "machine", machine: "b" });
    render(Machine, { props: { machine: "b" }, context: new Map([[STORE_KEY, store]]) });
    await tick();
    return { store, act, push: (r: ResourceSample) => onRes?.("b", r) };
  }

  it("the page asks for live resources and draws them with each sample", async () => {
    const { act, push } = await mountLive();
    expect(act).toHaveBeenCalledWith({ kind: "watch_resources", on: true });
    expect(screen.getByText("Ryzen 9 7950X · 32 threads")).toBeInTheDocument();
    expect(screen.getByText("RTX 4090")).toBeInTheDocument();
    push({ cpu: { load: 425, cores: [], temperature: 55 }, memory: { used: 32 * GIB, total: 64 * GIB }, gpus: [], disk: { free: 100 * GIB, total: 500 * GIB } });
    await tick();
    expect(screen.getByText("43 %")).toBeInTheDocument();
    expect(screen.getByText("55 °C")).toBeInTheDocument();
    expect(screen.getByText("100 GB free of 500 GB")).toBeInTheDocument();
    // Protected screens cannot be turned off and it says why.
    expect(screen.getByText("The administrator service is missing")).toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: "Protected screens" })).toBeNull();
  });

  it("each action goes to the agent with its command", async () => {
    const { act } = await mountLive();
    await userEvent.click(screen.getByRole("switch", { name: "Clipboard" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_module", machine: "b", module: "clipboard", on: false });
    await userEvent.click(screen.getByRole("radio", { name: "Teal" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "b", color: "#5fbfb0" });
    await userEvent.click(screen.getByRole("button", { name: "Scan again" }));
    expect(act).toHaveBeenCalledWith({ kind: "rescan", machine: "b" });
    await userEvent.click(screen.getByRole("button", { name: /Workshop/ }));
    await userEvent.type(screen.getByRole("textbox", { name: "Machine name" }), "{Control>}a{/Control}New{Enter}");
    expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "b", name: "New" });
    await userEvent.click(screen.getByRole("switch", { name: "Allow remote execution" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_module", machine: "b", module: "tunnels", on: false });
    await userEvent.click(screen.getByRole("button", { name: "Power off" }));
    expect(act).toHaveBeenCalledWith({ kind: "set_power", machine: "b", on: false });
  });

  it("an agent rejection is shown with its reason", async () => {
    const { store, act } = await mountLive();
    act.mockResolvedValueOnce({ kind: "rejected", reason: "that color is already the one of Studio" } as never);
    await userEvent.click(screen.getByRole("radio", { name: "Amber" }));
    await vi.waitFor(() => expect(store.toasts[0]).toMatchObject({ tone: "bad", text: "that color is already the one of Studio" }));
  });

  it("on leaving the page it stops asking for resources", async () => {
    const { store, act } = await mountLive();
    store.go({ screen: "settings" });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_resources", on: false });
    // Network does not show resources: it releases them too, and asks for its own topic (T-118).
    store.go({ screen: "machine", machine: "a" });
    store.go({ screen: "network" });
    expect(act).toHaveBeenCalledWith({ kind: "watch_resources", on: false });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_network", on: true });
  });
});
