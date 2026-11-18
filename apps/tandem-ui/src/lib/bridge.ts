// Cómo habla la ventana con el núcleo de la app (y él con tandemd).

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
  /** Muestras de recursos en vivo (solo tras `watch_resources`). */
  onResources(cb: (machine: Machine, r: ResourceSample) => void): Promise<void>;
  /** El estado de la red, una vez por segundo (solo tras `watch_network`). */
  onNetwork(cb: (n: NetworkWire) => void): Promise<void>;
  /** Los túneles por proyecto: al conectar y con cada cambio (T-114). */
  onTunnels(cb: (t: ProjectWire[]) => void): Promise<void>;
  /** El avance de las copias de archivos de los túneles (T-106). */
  onTransfer(cb: (t: TransferWire) => void): Promise<void>;
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
  /** «Salir» desde la bandeja: cierra la app (el agente sigue). */
  quit(): Promise<void>;
  /** La guía de permisos del primer arranque (macOS). */
  setup(): Promise<SetupView>;
  /** Abre el panel de Ajustes del Sistema que concede el permiso. */
  openSettings(permission: Permission): Promise<void>;
  /** Reinicia `tandemd` (launchd lo vuelve a levantar) para que valgan los permisos recién dados. */
  restartAgent(): Promise<void>;
  /** El estado real del servicio de administrador (T-131). */
  adminStatus(): Promise<AdminView>;
  /** «Instalar»: diálogo de administrador (macOS) o UAC (Windows). Rechaza con la razón. */
  installAdmin(): Promise<AdminView>;
  /** Marca la guía como vista: no vuelve a abrirse sola. */
  finishSetup(): Promise<SetupView>;
  /** Ajustes → Actualizaciones (T-122): versiones, búsqueda y «Actualizar solo». */
  updates(): Promise<UpdateView>;
  onUpdates(cb: (view: UpdateView) => void): Promise<void>;
  checkUpdates(): Promise<UpdateView>;
  /** «Actualizar todas»: rechaza con la razón si no se puede empezar. */
  updateAll(): Promise<UpdateView>;
  setAutoUpdate(on: boolean): Promise<UpdateView>;
  /** Ajustes → Avanzado: dónde están la configuración y los logs, y mostrarlos. */
  files(): Promise<{ config: string; logs: string }>;
  openFiles(target: "config" | "logs"): Promise<void>;
}

// Nombres de `tandem_ui::app`.
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

/** Lo que se ve si ni siquiera se pudo hablar con el núcleo (por ejemplo, fuera de Tauri). */
export function bridgeFailed(err: unknown): Snapshot {
  const detail = err instanceof Error ? err.message : String(err);
  return {
    kind: "unavailable",
    endpoint: null,
    reason: `la ventana no pudo preguntarle al núcleo de la app: ${detail}`,
  };
}
