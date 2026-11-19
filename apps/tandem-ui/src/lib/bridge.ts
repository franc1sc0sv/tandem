// How the window talks to the app core (and the core to tandemd).

import type { Action, ActionResult, AdminView, Bindings, Machine, Notice, Permission, ResourceSample, SetupView, ShortcutsView, Snapshot } from "./model";
import type { NetworkWire } from "./network";
import type { ProjectWire, TransferWire } from "./tunnels";
import type { UpdateView } from "./updates";

export interface Bridge {
  snapshot(): Promise<Snapshot>;
  act(action: Action): Promise<ActionResult>;
  retry(): Promise<void>;
  onSnapshot(cb: (s: Snapshot) => void): Promise<void>;
  onNotice(cb: (n: Notice) => void): Promise<void>;
  /** Live resource samples (only after `watch_resources`). */
  onResources(cb: (machine: Machine, r: ResourceSample) => void): Promise<void>;
  /** The network state, once per second (only after `watch_network`). */
  onNetwork(cb: (n: NetworkWire) => void): Promise<void>;
  /** Tunnels per project: on connect and on every change (T-114). */
  onTunnels(cb: (t: ProjectWire[]) => void): Promise<void>;
  /** Progress of the tunnels' file copies (T-106). */
  onTransfer(cb: (t: TransferWire) => void): Promise<void>;
  /** What happened with a request made from outside the window (global shortcut, tray menu). */
  onMessage(cb: (text: string) => void): Promise<void>;
  shortcuts(): Promise<ShortcutsView>;
  /** The new shortcuts every time they change (from any window). */
  onShortcuts(cb: (view: ShortcutsView) => void): Promise<void>;
  /** Rejects with the reason if they cannot be used (duplicates, invalid, unable to save). */
  setShortcuts(bindings: Bindings): Promise<ShortcutsView>;
  /** From the tray's mini scene. */
  showMain(): Promise<void>;
  /** The show/hide shortcut, handled by the main window in front. */
  hideMain(): Promise<void>;
  hideTray(): Promise<void>;
  /** "Quit" from the tray: closes the app (the agent keeps running). */
  quit(): Promise<void>;
  /** The first-run permissions guide (macOS). */
  setup(): Promise<SetupView>;
  /** Opens the System Settings panel that grants the permission. */
  openSettings(permission: Permission): Promise<void>;
  /** Restarts `tandemd` (launchd brings it back up) so the permissions you just gave take effect. */
  restartAgent(): Promise<void>;
  /** The real state of the administrator service (T-131). */
  adminStatus(): Promise<AdminView>;
  /** "Install": administrator dialog (macOS) or UAC (Windows). Rejects with the reason. */
  installAdmin(): Promise<AdminView>;
  /** Marks the guide as seen: it does not open by itself again. */
  finishSetup(): Promise<SetupView>;
  /** Settings → Updates (T-122): versions, search and "Update automatically". */
  updates(): Promise<UpdateView>;
  onUpdates(cb: (view: UpdateView) => void): Promise<void>;
  checkUpdates(): Promise<UpdateView>;
  /** "Update all": rejects with the reason if it cannot start. */
  updateAll(): Promise<UpdateView>;
  setAutoUpdate(on: boolean): Promise<UpdateView>;
  /** Settings → Advanced: where the configuration and the logs are, and showing them. */
  files(): Promise<{ config: string; logs: string }>;
  openFiles(target: "config" | "logs"): Promise<void>;
}

// Names from `tandem_ui::app`.
const SNAPSHOT_EVENT = "agent-snapshot";
const NOTICE_EVENT = "agent-notice";
const RESOURCES_EVENT = "agent-resources";
const NETWORK_EVENT = "agent-network";
const TUNNELS_EVENT = "agent-tunnels";
const TRANSFER_EVENT = "agent-transfer";
const MESSAGE_EVENT = "ui-message";
const SHORTCUTS_EVENT = "ui-shortcuts";
const UPDATE_EVENT = "ui-update";

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
    onResources: async (cb) => {
      await listen<{ machine: Machine; resources: ResourceSample }>(RESOURCES_EVENT, (e) => cb(e.payload.machine, e.payload.resources));
    },
    onNetwork: async (cb) => {
      await listen<NetworkWire>(NETWORK_EVENT, (e) => cb(e.payload));
    },
    onTunnels: async (cb) => {
      await listen<ProjectWire[]>(TUNNELS_EVENT, (e) => cb(e.payload));
    },
    onTransfer: async (cb) => {
      await listen<TransferWire>(TRANSFER_EVENT, (e) => cb(e.payload));
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
    quit: () => invoke<void>("ui_quit"),
    setup: () => invoke<SetupView>("setup_get"),
    openSettings: (permission) => invoke<void>("setup_open", { permission }),
    restartAgent: () => invoke<void>("agent_restart"),
    adminStatus: () => invoke<AdminView>("admin_status"),
    installAdmin: () => invoke<AdminView>("admin_install"),
    finishSetup: () => invoke<SetupView>("setup_finish"),
    updates: () => invoke<UpdateView>("update_get"),
    onUpdates: async (cb) => {
      await listen<UpdateView>(UPDATE_EVENT, (e) => cb(e.payload));
    },
    checkUpdates: () => invoke<UpdateView>("update_check"),
    updateAll: () => invoke<UpdateView>("update_all"),
    setAutoUpdate: (on) => invoke<UpdateView>("update_auto", { on }),
    files: () => invoke<{ config: string; logs: string }>("files_info"),
    openFiles: (target) => invoke<void>("files_open", { target }),
  };
}

/** What is shown if the core could not even be reached (for example, outside Tauri). */
export function bridgeFailed(err: unknown): Snapshot {
  const detail = err instanceof Error ? err.message : String(err);
  return {
    kind: "unavailable",
    endpoint: null,
    reason: `the window could not ask the app core: ${detail}`,
  };
}
