// Cómo habla la ventana con el núcleo de la app (y él con tandemd).

import type { Action, ActionResult, Notice, Snapshot } from "./model";

export interface Bridge {
  snapshot(): Promise<Snapshot>;
  act(action: Action): Promise<ActionResult>;
  retry(): Promise<void>;
  onSnapshot(cb: (s: Snapshot) => void): Promise<void>;
  onNotice(cb: (n: Notice) => void): Promise<void>;
}

// Nombres de `tandem_ui::app`.
const SNAPSHOT_EVENT = "agent-snapshot";
const NOTICE_EVENT = "agent-notice";

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
