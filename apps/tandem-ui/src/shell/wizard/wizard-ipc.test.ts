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

  it("paso 4: la actividad de esta ventana se marca y se le avisa al agente con `heard_key`", async () => {
    const { dlg, act, calls } = await mount(3);
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    await fireEvent.keyDown(window, { key: "a" });
    expect(await within(mac).findByText("Actividad detectada")).toBeInTheDocument();
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "heard_key" }));
    await fireEvent.keyDown(window, { key: "b" });
    expect(calls.filter((c) => c.kind === "heard_key")).toHaveLength(1);
  });

  it("paso 4: solo lista el teclado y el mouse si el agente anuncia el hub aquí", async () => {
    const sin = await mount(3);
    const macSin = within(sin.dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(macSin).getByText("Sin teclado ni mouse ahora")).toBeInTheDocument();
    expect(within(macSin).queryByText("Teclado y mouse")).toBeNull();
    sin.unmount();
    const base = snapshot();
    const con = await mount(3, snapshot({ caps: { ...base.caps, here: { ...base.caps.here, hub: true } } }));
    const macCon = within(con.dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(macCon).getByText("Teclado y mouse")).toBeInTheDocument();
    expect(within(macCon).queryByText("Sin teclado ni mouse ahora")).toBeNull();
  });

  it("«Cerrar» existe con nombre, cierra con un clic y su CSS le da caja de 28 px o más", async () => {
    const { dlg, store } = await mount(1);
    const close = within(dlg).getByRole("button", { name: "Cerrar" });
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
    expect(within(dlg).getAllByText("Actividad detectada").length).toBeGreaterThan(0);
  });

  it("paso 4: la fila de la otra máquina también sale de su hub", async () => {
    const base = snapshot();
    const peer = { ...base.caps.here, machine: "b" as const, name: "PC-Escritorio", os: "windows" as const, hub: true };
    const { dlg } = await mount(3, snapshot({ caps: { ...base.caps, peer } }));
    expect(within(within(dlg).getByRole("group", { name: "PC-Escritorio" })).getByText("Teclado y mouse")).toBeInTheDocument();
    expect(within(within(dlg).getByRole("group", { name: "MacBook Pro" })).getByText("Sin teclado ni mouse ahora")).toBeInTheDocument();
  });

  it("paso 5: mover la otra máquina guarda el acomodo con `set_arrangement`", async () => {
    const { dlg, act } = await mount(4);
    const other = within(dlg).getByRole("button", { name: /Mover PC-Escritorio/ });
    other.focus();
    await userEvent.keyboard("{ArrowDown}");
    await waitFor(() => expect(act).toHaveBeenCalledWith(expect.objectContaining({ kind: "set_arrangement", a_side: "up" })));
  });

  it("paso 5: si el agente no informa el cruce, la prueba lo dice y deja saltar", async () => {
    const { dlg, store } = await mount(4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }));
    expect(await within(dlg).findByText(/todavía no informa el cruce/)).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(false);
    await userEvent.click(within(dlg).getByRole("button", { name: "Saltar" }));
    expect(store.wizardStep).toBe(5);
  });
});
