// La guía de permisos del primer arranque en macOS: una pantalla de bienvenida (con cómo
// quedó tandemd) y una por permiso. Aquí, lo que no depende del DOM.

import type { Permission, SetupView } from "./model";

export interface PermissionInfo {
  title: string;
  /** Para qué lo usa Tandem. */
  why: string;
  /** Qué hacer en Ajustes del Sistema. */
  how: string;
}

// Los permisos son de `tandemd` (es quien captura e inyecta): la app solo guía.
const SIDECAR = "/Applications/Tandem.app/Contents/MacOS/tandemd";

export const PERMISSION_INFO: Record<Permission, PermissionInfo> = {
  accessibility: {
    title: "Accessibility",
    why: "To type and move the mouse on this machine when you use them from the other one.",
    how: `In System Settings → Privacy & Security → Accessibility, turn on tandemd. First press “Request permissions”: that way it shows up on its own. If not, press + and choose ${SIDECAR}.`,
  },
  input_monitoring: {
    title: "Input Monitoring",
    why: "To carry the keyboard and mouse to the other machine when the cursor crosses the edge.",
    how: `In System Settings → Privacy & Security → Input Monitoring, turn on tandemd. First press “Request permissions”: that way it shows up on its own. If not, press + and choose ${SIDECAR}.`,
  },
  local_network: {
    title: "Local Network",
    why: "To find the other machine on the network and talk to it encrypted.",
    how: "In System Settings → Privacy & Security → Local Network, turn on Tandem and tandemd. If they do not show up yet, macOS will ask the first time they look for the other machine.",
  },
};

/** Las pantallas de la guía: la bienvenida y una por permiso. */
export type Screen = { kind: "intro" } | { kind: "permission"; permission: Permission };

export function screensOf(view: SetupView): Screen[] {
  return [{ kind: "intro" }, ...view.permissions.map((permission) => ({ kind: "permission" as const, permission }))];
}

/** Dónde está la guía. `index` siempre es una pantalla válida. */
export interface Wizard {
  index: number;
  total: number;
}

export function start(view: SetupView): Wizard {
  return { index: 0, total: screensOf(view).length };
}

export function next(w: Wizard): Wizard {
  return { ...w, index: Math.min(w.index + 1, w.total - 1) };
}

export function back(w: Wizard): Wizard {
  return { ...w, index: Math.max(w.index - 1, 0) };
}

export const isFirst = (w: Wizard) => w.index === 0;
export const isLast = (w: Wizard) => w.index >= w.total - 1;
