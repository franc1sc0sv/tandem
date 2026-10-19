// Los permisos de macOS tal como el usuario los ve: el nombre en español y el del panel en
// inglés (la Mac puede estar en inglés), y a qué binario se conceden (T-090). Sin DOM.

import type { MachineCaps, Permission } from "./model";

export const PERMISSION_NAMES: Record<Permission, { es: string; en: string }> = {
  accessibility: { es: "Accesibilidad", en: "Accessibility" },
  input_monitoring: { es: "Monitorización de entrada", en: "Input Monitoring" },
  local_network: { es: "Red local", en: "Local Network" },
};

/** «Accesibilidad (Accessibility)»: el español y, entre paréntesis, el nombre del panel en inglés. */
export const permissionLabel = (p: Permission) => `${PERMISSION_NAMES[p].es} (${PERMISSION_NAMES[p].en})`;

/** Dónde se concede, en las dos lenguas del sistema. */
export const SETTINGS_PATH = "Ajustes del Sistema → Privacidad y seguridad (System Settings → Privacy & Security)";

/** El binario que corre el agente, o `null` si el agente no lo dijo. */
export function agentBinary(here: MachineCaps | null): string | null {
  const exe = here?.input.process.exe;
  return exe ? exe : null;
}

/** «Concede el permiso a este binario: <ruta>», con el pid para distinguir un reinicio. */
export function grantTo(here: MachineCaps | null): string | null {
  const exe = agentBinary(here);
  if (!exe || !here) return null;
  return `Concede el permiso a este binario: ${exe} (pid ${here.input.process.pid})`;
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
  "1. Pulsa «Pedir permisos»: macOS muestra su diálogo para este binario.",
  `2. Actívalo en ${SETTINGS_PATH}.`,
  "3. Pulsa «Reiniciar agente» para que valga el permiso recién dado.",
  "Si ya estaba activado y sigue faltando, quita la entrada vieja con «−», pulsa «Pedir permisos» y reinicia el agente.",
].join(" ");
