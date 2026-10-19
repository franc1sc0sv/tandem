import { beforeEach, expect, it, vi } from "vitest";

import type { ShortcutsView } from "./model";

const invoke = vi.fn(async (..._args: unknown[]) => undefined);
const handlers = new Map<string, (e: { payload: unknown }) => void>();

vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: (e: { payload: unknown }) => void) => {
    handlers.set(name, cb);
    return () => handlers.delete(name);
  },
}));

const { bridgeFailed, tauriBridge } = await import("./bridge");

beforeEach(() => {
  invoke.mockClear();
  handlers.clear();
});

it("convierte un fallo del puente en un estado sin agente", () => {
  expect(bridgeFailed(new Error("sin __TAURI_INTERNALS__"))).toEqual({
    kind: "unavailable",
    endpoint: null,
    reason: "la ventana no pudo preguntarle al núcleo de la app: sin __TAURI_INTERNALS__",
  });
});

it("recibe los atajos nuevos por el evento de `tandem_ui::app`", async () => {
  const bridge = await tauriBridge();
  const seen: ShortcutsView[] = [];
  await bridge.onShortcuts((v) => seen.push(v));
  const view: ShortcutsView = {
    bindings: { solo_mac: "Ctrl+Alt+KeyM", shared: null, solo_pc: null, toggle_window: null },
    failed: {},
    warning: null,
    file: null,
  };
  handlers.get("ui-shortcuts")?.({ payload: view });
  expect(seen).toEqual([view]);
});

it("esconde la ventana principal con su comando", async () => {
  const bridge = await tauriBridge();
  await bridge.hideMain();
  expect(invoke).toHaveBeenCalledWith("ui_hide_main");
});

it("usa los comandos de la guía de permisos de `tandem_ui::app`", async () => {
  const bridge = await tauriBridge();
  await bridge.setup();
  await bridge.openSettings("input_monitoring");
  await bridge.restartAgent();
  await bridge.finishSetup();
  expect(invoke.mock.calls).toEqual([
    ["setup_get"],
    ["setup_open", { permission: "input_monitoring" }],
    ["agent_restart"],
    ["setup_finish"],
  ]);
});
