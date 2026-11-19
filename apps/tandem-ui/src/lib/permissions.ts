// macOS permissions as the user sees them: the name shown in the Settings panel, and which
// binary they are granted to (T-090). No DOM.

import type { ActionResult, MachineCaps, Permission } from "./model";

export const PERMISSION_NAMES: Record<Permission, string> = {
  accessibility: "Accessibility",
  input_monitoring: "Input Monitoring",
  local_network: "Local Network",
};

/** The permission name as the Settings panel shows it. */
export const permissionLabel = (p: Permission) => `${PERMISSION_NAMES[p]}`;

/** Where it is granted. */
export const SETTINGS_PATH = "System Settings → Privacy & Security";

/** El binario que corre el agente, o `null` si el agente no lo dijo. */
export function agentBinary(here: MachineCaps | null): string | null {
  const exe = here?.input.process.exe;
  return exe ? exe : null;
}

/** «Concede el permiso a este binario: <ruta>», con el pid para distinguir un reinicio. */
export function grantTo(here: MachineCaps | null): string | null {
  const exe = agentBinary(here);
  if (!exe || !here) return null;
  return `Grant the permission to this binary: ${exe} (pid ${here.input.process.pid})`;
}

/** Lo que sabe el agente de un permiso: `unknown` si no hay agente o no lo comprueba (red local). */
export type PermissionState = "granted" | "missing" | "unknown";

export function permissionState(here: MachineCaps | null, p: Permission): PermissionState {
  if (!here || p === "local_network") return "unknown";
  return here.input.missing.includes(p) ? "missing" : "granted";
}

/**
 * Qué hacer cuando falta algo. macOS evalúa parte de los permisos una vez por proceso, y la
 * entrada de la lista es de un binario concreto: si ya estaba activada pero sigue faltando,
 * casi siempre es una entrada vieja (otra versión o ruta) y hay que quitarla y volver a
 * pedirla.
 */
export const MISSING_HELP = [
  "1. Press “Request permissions”: macOS shows its dialog for this binary.",
  `2. Turn it on in ${SETTINGS_PATH}.`,
  "3. Press “Restart agent” so the permission you just gave takes effect.",
  "If it was already on and is still missing, remove the old entry with “−”, press “Request permissions” and restart the agent.",
].join(" ");

/** Lo que hacen los botones de permisos, sin DOM: cada uno devuelve el error a mostrar y el notice. */
export interface PermissionDeps {
  request(): Promise<ActionResult>;
  /** `null` fuera de la app (no hay con qué reiniciar el agente). */
  restartAgent: (() => Promise<void>) | null;
  openSettings: (() => Promise<void>) | null;
}

export interface PermissionOutcome {
  error: string | null;
  message?: string;
}

export async function pressPermission(action: string, d: PermissionDeps): Promise<PermissionOutcome> {
  if (action === "request") {
    const r = await d.request();
    return { error: r.kind === "rejected" ? r.reason : r.kind === "unavailable" ? `no agent: ${r.reason}` : null };
  }
  if (action === "restart") {
    if (!d.restartAgent) return { error: "Could not restart the agent: the app is not available" };
    try {
      await d.restartAgent();
      return { error: null, message: "tandemd is restarting: permissions are re-checked in a few seconds" };
    } catch (e) {
      return { error: `Could not restart the agent: ${e instanceof Error ? e.message : String(e)}` };
    }
  }
  await d.openSettings?.();
  return { error: null };
}
