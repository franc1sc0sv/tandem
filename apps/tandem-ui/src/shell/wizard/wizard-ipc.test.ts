import { fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
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

/** Una Mac (esta máquina) con Accesibilidad sin conceder y una PC emparejada por USB-C. */
function snapshot(over: Partial<Connected> = {}): Connected {
  return {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: null, b: { os: "windows", name: "PC-Escritorio", color: "#a99be8" } }, screens: [], arrangement: { a_side: "right", align: "start" }, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Escritorio", link: "usb4" },
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

async function mount(step: number, snap: Connected = snapshot()) {
  const calls: Action[] = [];
  const act = vi.fn(async (a: Action): Promise<ActionResult> => {
    calls.push(a);
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
  const store = new Store(new LiveSource(bridge), { hash: "#/puesto" });
  await store.start();
  store.openWizard(step);
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  const dlg = screen.getByRole("dialog", { name: "Asistente" });
  const kinds = () => calls.map((c) => c.kind);
  return { ...r, store, dlg, act, calls, kinds, openSettings, restartAgent, adminStatus, installAdmin, notice: (n: Notice) => notice?.(n), push: (s: Snapshot) => push?.(s) };
}

describe("el asistente contra el agente (IPC real)", () => {
  it("paso 1: el nombre y el color se guardan con `set_machine`", async () => {
    const { dlg, act } = await mount(0);
    const input = within(dlg).getByRole("textbox", { name: "Nombre" });
    await userEvent.clear(input);
    await userEvent.type(input, "Portátil{Enter}");
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "a", name: "Portátil" }));
    await userEvent.click(within(dlg).getByRole("radio", { name: "Ámbar" }));
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "set_machine", machine: "a", color: "#d9a75a" }));
  });

  it("paso 1: «Instalar» llama al puente y la fila toma el estado que devuelve", async () => {
    const { dlg, installAdmin } = await mount(0);
    const found = within(dlg).getByRole("region", { name: "Lo que encontró" });
    await userEvent.click(await within(found).findByRole("button", { name: "Instalar" }));
    expect(installAdmin).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(within(found).queryByRole("button", { name: "Instalar" })).toBeNull());
  });

  it("paso 1: si el usuario cancela, avisa y la fila vuelve a su estado real sin girar", async () => {
    const { dlg, installAdmin, adminStatus } = await mount(0);
    installAdmin.mockRejectedValueOnce(new Error("se canceló la petición de administrador"));
    const found = within(dlg).getByRole("region", { name: "Lo que encontró" });
    await userEvent.click(await within(found).findByRole("button", { name: "Instalar" }));
    expect(await screen.findByText("se canceló la petición de administrador")).toBeInTheDocument();
    await waitFor(() => expect(adminStatus.mock.calls.length).toBeGreaterThan(1));
    expect(within(found).getByRole("button", { name: "Instalar" })).toBeEnabled();
  });

  it("paso 2: sin emparejar busca sola y confirma el código que escribe el usuario", async () => {
    const { dlg, act, push, kinds } = await mount(1, snapshot({ peer: { kind: "unpaired" } }));
    await waitFor(() => expect(kinds()).toContain("start_pairing"));
    push(snapshot({ peer: { kind: "unpaired" }, pairing: { kind: "comparing", code: "418027", peer: "PC-Escritorio", confirmed: false } }));
    await tick();
    for (const [i, d] of [..."418027"].entries()) await userEvent.type(within(dlg).getByRole("textbox", { name: `Dígito ${i + 1}` }), d);
    await waitFor(() => expect(act).toHaveBeenLastCalledWith({ kind: "confirm_pairing" }));
  });

  it("paso 2: «Cambiar la otra máquina» desempareja con `unpair` solo tras confirmar", async () => {
    const { dlg, kinds } = await mount(1);
    await userEvent.click(within(dlg).getByRole("button", { name: "Cambiar la otra máquina" }));
    expect(kinds()).not.toContain("unpair");
    await userEvent.click(within(dlg).getByRole("button", { name: "Desemparejar" }));
    await waitFor(() => expect(kinds()).toContain("unpair"));
  });

  it("paso 3: abre el panel del permiso y reinicia el agente por el puente", async () => {
    const { dlg, openSettings, restartAgent } = await mount(2);
    expect(within(dlg).getByRole("heading", { name: "Permisos de macOS" })).toBeInTheDocument();
    const next = within(dlg).getByRole("button", { name: "Continuar" });
    expect(next).toHaveClass("wait");
    await userEvent.click(within(dlg).getAllByRole("button", { name: "Abrir ajustes" })[0]!);
    expect(openSettings).toHaveBeenCalledWith("accessibility");
    await userEvent.click(await within(dlg).findByRole("button", { name: "Reiniciar agente" }));
    expect(restartAgent).toHaveBeenCalledTimes(1);
  });

  it("paso 3: cuando el agente informa el permiso concedido, Continuar se habilita", async () => {
    const { dlg, push } = await mount(2);
    expect(within(dlg).getByRole("button", { name: "Continuar" })).toHaveClass("wait");
    const ok = snapshot();
    ok.input = { ...ok.input, missing: [] };
    ok.caps = { ...ok.caps, here: { ...ok.caps.here, input: { ...ok.caps.here.input, capture: true, inject: true, missing: [] } } };
    push(ok);
    await tick();
    await waitFor(() => expect(within(dlg).getByRole("button", { name: "Continuar" })).not.toHaveClass("wait"));
  });

  it("paso 4: enciende la sonda de teclas, marca la máquina que oyó y la apaga al salir", async () => {
    const { dlg, notice, act, unmount } = await mount(3);
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "key_probe", on: true }));
    const pc = within(dlg).getByRole("group", { name: "PC-Escritorio" });
    expect(within(pc).queryByText("Actividad detectada")).toBeNull();
    notice({ kind: "key_probe", event: { armed: true, heard: "b" } });
    expect(await within(pc).findByText("Actividad detectada")).toBeInTheDocument();
    unmount();
    await waitFor(() => expect(act).toHaveBeenLastCalledWith({ kind: "key_probe", on: false }));
  });

  it("paso 4: la actividad de esta ventana se marca sin pedirle nada al agente", async () => {
    const { dlg } = await mount(3);
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    await fireEvent.keyDown(window, { key: "a" });
    expect(await within(mac).findByText("Actividad detectada")).toBeInTheDocument();
  });

  it("paso 5: mover la otra máquina guarda el acomodo con `set_arrangement`", async () => {
    const { dlg, act } = await mount(4);
    const other = within(dlg).getByRole("button", { name: /Mover PC-Escritorio/ });
    other.focus();
    await userEvent.keyboard("{ArrowDown}");
    await waitFor(() => expect(act).toHaveBeenCalledWith(expect.objectContaining({ kind: "set_arrangement", a_side: "up" })));
  });

  it("paso 5: «Probar cruce» pide el cursor al agente y un Crossed por el borde marcado lo da por bueno", async () => {
    const { dlg, store, act, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }));
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true }));
    notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
    notice({ kind: "cursor", sample: { machine: "b", screen: "m34", x: 3, y: 720 } });
    expect(await within(dlg).findByText(/Cruzó a PC-Escritorio por el borde izquierdo, como dice el acomodo/)).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(true);
  });

  it("paso 5: un Crossed por otro borde da «Cruzó por otro borde» y no habilita seguir", async () => {
    const { dlg, store, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }));
    await waitFor(() => expect(store.wizardCrossed).toBe(false));
    notice({ kind: "crossed", from: "a", to: "b", edge: "right" });
    expect(await within(dlg).findByText(/Cruzó por la derecha, no por la izquierda/, {}, { timeout: 2000 })).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(false);
  });

  it("paso 5: un Crossed sin muestra del cursor pasa sin posición ni latencia inventada", async () => {
    const { dlg, store, notice } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }));
    await waitFor(() => expect(store.wizardCrossed).toBe(false));
    notice({ kind: "crossed", from: "a", to: "b", edge: "left" });
    const text = await within(dlg).findByText(/Cruzó a PC-Escritorio por el borde izquierdo, como dice el acomodo/, {}, { timeout: 2000 });
    expect(text.textContent).not.toMatch(/Vuelta medida/);
    expect(store.wizardCrossed).toBe(true);
  });

  it("paso 5: sin ningún cruce en 20 s la prueba dice «No cruzó» y no habilita seguir", async () => {
    const { dlg, store, act } = await mount(4);
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    try {
      await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }), { advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
      await vi.advanceTimersByTimeAsync(20_500);
      expect(await within(dlg).findByText(/No cruzó en 20 s/)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
    expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true });
    expect(store.wizardCrossed).toBe(false);
  });
});
