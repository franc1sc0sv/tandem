import { fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import type { Bridge } from "../../lib/bridge";
import { STORE_KEY } from "../../lib/context";
import type { Action, ActionResult, AdminView, Notice, Snapshot } from "../../lib/model";
import { LiveSource } from "../../lib/store/live";
import { Store } from "../../lib/store/store.svelte";
import Shell from "../Shell.svelte";

// El asistente contra la fuente real (`LiveSource`) y un puente simulado: lo que el usuario hace
// en cada paso llega al agente como el pedido de IPC que corresponde.

type Connected = Extract<Snapshot, { kind: "connected" }>;

/** Una Mac (this machine) con Accesibilidad sin conceder y una PC emparejada por USB-C. */
function snapshot(over: Partial<Connected> = {}): Connected {
  return {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: null, b: { os: "windows", name: "PC-Desktop", color: "#a99be8" } }, screens: [], arrangement: { a_side: "right", align: "start" }, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Desktop", link: "usb4" },
    pairing: { kind: "idle" },
    monitors: [],
    input: { server: "a", hub_here: null, session: true, locked: false, missing: ["accessibility"], problem: null, defined: true },
    caps: {
      here: {
        machine: "a",
        name: "MacBook Pro",
        os: "macos",
        color: "#7aa7e6",
        hub: null,
        input: { capture: false, inject: false, missing: ["accessibility"], process: { exe: "/x/tandemd", pid: 7, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: false },
        services: [],
      },
      peer: null,
    },
    ...over,
  };
}

async function mount(step: number, snap: Connected = snapshot(), onAct?: (a: Action) => Promise<void>) {
  const calls: Action[] = [];
  const act = vi.fn(async (a: Action): Promise<ActionResult> => {
    if (onAct) await onAct(a);
    else calls.push(a);
    return { kind: "done" };
  });
  const openSettings = vi.fn(async () => {});
  const restartAgent = vi.fn(async () => {});
  let adminState: AdminView = { state: "pending", canInstall: true, note: "" };
  const adminStatus = vi.fn(async () => adminState);
  const installAdmin = vi.fn(async () => {
    adminState = { state: "ok", canInstall: false, note: "" };
    return adminState;
  });
  let notice: ((n: Notice) => void) | undefined;
  let push: ((s: Snapshot) => void) | undefined;
  const bridge = {
    snapshot: async () => snap,
    onSnapshot: async (cb: (s: Snapshot) => void) => {
      push = cb;
    },
    onNotice: async (cb: (n: Notice) => void) => {
      notice = cb;
    },
    shortcuts: async () => ({ bindings: {}, failed: {}, warning: null, file: null }),
    onShortcuts: async () => {},
    onResources: async () => {},
    onNetwork: async () => {},
    onTunnels: async () => {},
    onTransfer: async () => {},
    onMessage: async () => {},
    act,
    openSettings,
    restartAgent,
    adminStatus,
    installAdmin,
  } as unknown as Bridge;
  const store = new Store(new LiveSource(bridge), { hash: "#/desk" });
  await store.start();
  store.openWizard(step);
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  const dlg = screen.getByRole("dialog", { name: "Setup" });
  const kinds = () => calls.map((c) => c.kind);
  return { ...r, store, dlg, act, calls, kinds, openSettings, restartAgent, adminStatus, installAdmin, notice: (n: Notice) => notice?.(n), push: (s: Snapshot) => push?.(s) };
}

describe("el asistente contra el agente (IPC real)", () => {
  it("paso 1: el nombre y el color se guardan con `set_machine`", async () => {
    const { dlg, act } = await mount(0);
    const input = within(dlg).getByRole("textbox", { name: "Name" });
    await userEvent.clear(input);
    await userEvent.type(input, "Portátil{Enter}");
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "a", name: "Portátil" }));
    await userEvent.click(within(dlg).getByRole("radio", { name: "Amber" }));
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "a", color: "#d9a75a" }));
  });

  it("paso 1: «Instalar» llama al puente y la fila toma el estado que devuelve", async () => {
    const { dlg, installAdmin } = await mount(0);
    const found = within(dlg).getByRole("region", { name: "What it found" });
    await userEvent.click(await within(found).findByRole("button", { name: "Install" }));
    expect(installAdmin).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(within(found).queryByRole("button", { name: "Install" })).toBeNull());
  });

  it("paso 1: si el usuario cancela, avisa y la fila vuelve a su estado real sin girar", async () => {
    const { dlg, installAdmin, adminStatus } = await mount(0);
    installAdmin.mockRejectedValueOnce(new Error("se canceló la petición de administrador"));
    const found = within(dlg).getByRole("region", { name: "What it found" });
    await userEvent.click(await within(found).findByRole("button", { name: "Install" }));
    expect(await screen.findByText("se canceló la petición de administrador")).toBeInTheDocument();
    await waitFor(() => expect(adminStatus.mock.calls.length).toBeGreaterThan(1));
    expect(within(found).getByRole("button", { name: "Install" })).toBeEnabled();
  });

  it("paso 2: sin emparejar busca sola y confirma el código que escribe el usuario", async () => {
    const { dlg, act, push, kinds } = await mount(1, snapshot({ peer: { kind: "unpaired" } }));
    await waitFor(() => expect(kinds()).toContain("start_pairing"));
    push(snapshot({ peer: { kind: "unpaired" }, pairing: { kind: "comparing", code: "418027", peer: "PC-Desktop", confirmed: false } }));
    await tick();
    for (const [i, d] of [..."418027"].entries()) await userEvent.type(within(dlg).getByRole("textbox", { name: `Digit ${i + 1}` }), d);
    await waitFor(() => expect(act).toHaveBeenLastCalledWith({ kind: "confirm_pairing" }));
  });

  it("paso 2: «Change the other machine» desempareja con `unpair` solo tras confirmar", async () => {
    const { dlg, kinds } = await mount(1);
    await userEvent.click(within(dlg).getByRole("button", { name: "Change the other machine" }));
    expect(kinds()).not.toContain("unpair");
    await userEvent.click(within(dlg).getByRole("button", { name: "Unpair" }));
    await waitFor(() => expect(kinds()).toContain("unpair"));
  });

  it("paso 3: abre el panel del permiso y reinicia el agente por el puente", async () => {
    const { dlg, openSettings, restartAgent } = await mount(2);
    expect(within(dlg).getByRole("heading", { name: "macOS permissions" })).toBeInTheDocument();
    const next = within(dlg).getByRole("button", { name: "Continue" });
    expect(next).toHaveClass("wait");
    await userEvent.click(within(dlg).getAllByRole("button", { name: "Open settings" })[0]!);
    expect(openSettings).toHaveBeenCalledWith("accessibility");
    await userEvent.click(await within(dlg).findByRole("button", { name: "Restart agent" }));
    expect(restartAgent).toHaveBeenCalledTimes(1);
  });

  it("paso 3: cuando el agente informa el permiso concedido, Continuar se habilita", async () => {
    const { dlg, push } = await mount(2);
    expect(within(dlg).getByRole("button", { name: "Continue" })).toHaveClass("wait");
    const ok = snapshot();
    ok.input = { ...ok.input, missing: [] };
    ok.caps = { ...ok.caps, here: { ...ok.caps.here, input: { ...ok.caps.here.input, capture: true, inject: true, missing: [] } } };
    push(ok);
    await tick();
    await waitFor(() => expect(within(dlg).getByRole("button", { name: "Continue" })).not.toHaveClass("wait"));
  });

  it("paso 4: enciende la sonda de teclas, marca la machine que oyó y la apaga al salir", async () => {
    const { dlg, notice, act, unmount } = await mount(3);
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "key_probe", on: true }));
    const pc = within(dlg).getByRole("group", { name: "PC-Desktop" });
    expect(within(pc).queryByText("Activity detected")).toBeNull();
    notice({ kind: "key_probe", event: { armed: true, heard: "b" } });
    expect(await within(pc).findByText("Activity detected")).toBeInTheDocument();
    unmount();
    await waitFor(() => expect(act).toHaveBeenLastCalledWith({ kind: "key_probe", on: false }));
  });

  it("paso 4: la actividad de esta ventana se marca y se le avisa al agente con `heard_key`", async () => {
    const { dlg, act, calls } = await mount(3);
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    await fireEvent.keyDown(window, { key: "a" });
    expect(await within(mac).findByText("Activity detected")).toBeInTheDocument();
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "heard_key" }));
    await fireEvent.keyDown(window, { key: "b" });
    expect(calls.filter((c) => c.kind === "heard_key")).toHaveLength(1);
  });

  it("paso 4: solo lista el teclado y el mouse si el agente anuncia el hub aquí", async () => {
    const sin = await mount(3);
    const macSin = within(sin.dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(macSin).getByText("No keyboard or mouse right now")).toBeInTheDocument();
    expect(within(macSin).queryByText("Keyboard and mouse")).toBeNull();
    sin.unmount();
    const base = snapshot();
    const con = await mount(3, snapshot({ caps: { ...base.caps, here: { ...base.caps.here, hub: true } } }));
    const macCon = within(con.dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(macCon).getByText("Keyboard and mouse")).toBeInTheDocument();
    expect(within(macCon).queryByText("No keyboard or mouse right now")).toBeNull();
  });

  it("«Cerrar» existe con nombre, cierra con un clic y su CSS le da caja de 28 px o más", async () => {
    const { dlg, store } = await mount(1);
    const close = within(dlg).getByRole("button", { name: "Close" });
    expect(close.textContent?.trim()).toBe("");
    const css = readFileSync(join(process.cwd(), "src", "shell", "Wizard.svelte"), "utf8");
    const rule = /\.wz-close \{([^}]*)\}/.exec(css)?.[1] ?? "";
    expect(Number(/width:\s*(\d+)px/.exec(rule)?.[1])).toBeGreaterThanOrEqual(28);
    expect(Number(/height:\s*(\d+)px/.exec(rule)?.[1])).toBeGreaterThanOrEqual(28);
    expect(rule).not.toContain("absolute");
    expect(css).toContain(".wz-close:focus-visible");
    await userEvent.click(close);
    expect(store.wizardOpen).toBe(false);
  });

  it("paso 4: la tecla que llega antes de armar la sonda se avisa al armarla", async () => {
    const calls: Action[] = [];
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => (release = r));
    const { dlg, act } = await mount(3, snapshot(), async (a) => {
      if (a.kind === "key_probe") await gate;
      calls.push(a);
    });
    await fireEvent.keyDown(window, { key: "a" });
    expect(calls.some((c) => c.kind === "heard_key")).toBe(false);
    release();
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "heard_key" }));
    expect(within(dlg).getAllByText("Activity detected").length).toBeGreaterThan(0);
  });

  it("paso 4: la fila de the other machine también sale de su hub", async () => {
    const base = snapshot();
    const peer = { ...base.caps.here, machine: "b" as const, name: "PC-Desktop", os: "windows" as const, hub: true };
    const { dlg } = await mount(3, snapshot({ caps: { ...base.caps, peer } }));
    expect(within(within(dlg).getByRole("group", { name: "PC-Desktop" })).getByText("Keyboard and mouse")).toBeInTheDocument();
    expect(within(within(dlg).getByRole("group", { name: "MacBook Pro" })).getByText("No keyboard or mouse right now")).toBeInTheDocument();
  });

  it("paso 5: mover the other machine guarda el arrangement con `set_arrangement`", async () => {
    const { dlg, act } = await mount(4);
    const other = within(dlg).getByRole("button", { name: /Move PC-Desktop/ });
    other.focus();
    await userEvent.keyboard("{ArrowDown}");
    await waitFor(() => expect(act).toHaveBeenCalledWith(expect.objectContaining({ kind: "set_arrangement", a_side: "up" })));
  });

  it("paso 5: «Test crossing» pide el cursor al agente y un Crossed por el borde marcado lo da por bueno", async () => {
    const { dlg, store, act, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true }));
    notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
    notice({ kind: "cursor", sample: { machine: "b", screen: "m34", x: 3, y: 720 } });
    expect(await within(dlg).findByText(/Crossed to PC-Desktop through the left edge, as the arrangement says/)).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(true);
  });

  it("paso 5: un Crossed por otro borde da «Crossed at another edge» y no habilita seguir", async () => {
    const { dlg, store, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    await waitFor(() => expect(store.wizardCrossed).toBe(false));
    notice({ kind: "crossed", from: "a", to: "b", edge: "right" });
    expect(await within(dlg).findByText(/Crossed at the right, not the left/, {}, { timeout: 2000 })).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(false);
  });

  it("paso 5: un Crossed sin muestra del cursor pasa sin posición ni latencia inventada", async () => {
    const { dlg, store, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    await waitFor(() => expect(store.wizardCrossed).toBe(false));
    notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
    const text = await within(dlg).findByText(/Crossed to PC-Desktop through the left edge, as the arrangement says/, {}, { timeout: 2000 });
    expect(text.textContent).not.toMatch(/Round trip measured/);
    expect(store.wizardCrossed).toBe(true);
  });

  it("paso 5: sin ningún crossing en 20 s la prueba dice «No cruzó» y no habilita seguir", async () => {
    const { dlg, store, act } = await mount(4);
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    try {
      await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }), { advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
      await vi.advanceTimersByTimeAsync(20_500);
      expect(await within(dlg).findByText(/Did not cross in 20 s/)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
    expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true });
    expect(store.wizardCrossed).toBe(false);
  });
});
