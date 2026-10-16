// Cómo habla la ventana con el núcleo de la app (y él con tandemd).

import type { Action, ActionResult, Bindings, Notice, ShortcutsView, Snapshot } from "./model";

export interface Bridge {
  snapshot(): Promise<Snapshot>;
  act(action: Action): Promise<ActionResult>;
  retry(): Promise<void>;
  onSnapshot(cb: (s: Snapshot) => void): Promise<void>;
  onNotice(cb: (n: Notice) => void): Promise<void>;
  /** Lo que pasó con un pedido hecho desde fuera de la ventana (atajo global, menú del ícono). */
  onMessage(cb: (text: string) => void): Promise<void>;
  shortcuts(): Promise<ShortcutsView>;
  /** Los atajos nuevos cada vez que se cambian (desde cualquier ventana). */
  onShortcuts(cb: (view: ShortcutsView) => void): Promise<void>;
  /** Rechaza con la razón si no se pueden usar (repetidos, inválidos, sin poder guardar). */
  setShortcuts(bindings: Bindings): Promise<ShortcutsView>;
  /** Desde la mini escena del ícono. */
  showMain(): Promise<void>;
  /** El atajo de mostrar u ocultar, atendido por la ventana principal al frente. */
  hideMain(): Promise<void>;
  hideTray(): Promise<void>;
}

// Nombres de `tandem_ui::app`.
const SNAPSHOT_EVENT = "agent-snapshot";
const NOTICE_EVENT = "agent-notice";
const MESSAGE_EVENT = "ui-message";
const SHORTCUTS_EVENT = "ui-shortcuts";

export async function tauriBridge(): Promise<Bridge> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  return {
    snapshot: () => invoke<Snapshot>("agent_snapshot"),
    act: (action) => invoke<ActionResult>("agent_act", { action }),
    retry: () => invoke<void>("agent_retry"),
    onSnapshot: async (cb) => {
      await listen<Snapshot>(SNAPSHOT_EVENT, (e) => cb(e.payload));
    },
    onNotice: async (cb) => {
      await listen<Notice>(NOTICE_EVENT, (e) => cb(e.payload));
    },
    onMessage: async (cb) => {
      await listen<string>(MESSAGE_EVENT, (e) => cb(e.payload));
    },
    shortcuts: () => invoke<ShortcutsView>("shortcuts_get"),
    onShortcuts: async (cb) => {
      await listen<ShortcutsView>(SHORTCUTS_EVENT, (e) => cb(e.payload));
    },
    setShortcuts: (bindings) => invoke<ShortcutsView>("shortcuts_set", { bindings }),
    showMain: () => invoke<void>("ui_show_main"),
    hideMain: () => invoke<void>("ui_hide_main"),
    hideTray: () => invoke<void>("ui_hide_tray"),
  };
}

/** Lo que se ve si ni siquiera se pudo hablar con el núcleo (por ejemplo, fuera de Tauri). */
export function bridgeFailed(err: unknown): Snapshot {
  const detail = err instanceof Error ? err.message : String(err);
  return {
    kind: "unavailable",
    endpoint: null,
    reason: `la ventana no pudo preguntarle al núcleo de la app: ${detail}`,
  };
}
