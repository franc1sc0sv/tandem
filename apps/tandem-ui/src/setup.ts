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
    title: "Accesibilidad (Accessibility)",
    why: "Para escribir y mover el mouse en la Mac cuando los usas desde la PC, y para traer ventanas de la pantalla integrada.",
    how: `En Ajustes del Sistema → Privacidad y seguridad → Accesibilidad (System Settings → Privacy & Security → Accessibility), activa tandemd. Primero pulsa «Pedir permisos»: así aparece solo. Si no, pulsa + y elige ${SIDECAR}.`,
  },
  input_monitoring: {
    title: "Monitorización de entrada (Input Monitoring)",
    why: "Para llevar el teclado y el mouse de la Mac a la PC cuando el cursor cruza el borde.",
    how: `En Ajustes del Sistema → Privacidad y seguridad → Monitorización de entrada (System Settings → Privacy & Security → Input Monitoring), activa tandemd. Primero pulsa «Pedir permisos»: así aparece solo. Si no, pulsa + y elige ${SIDECAR}.`,
  },
  local_network: {
    title: "Red local (Local Network)",
    why: "Para encontrar la PC en la red y hablar con ella cifrado (también por USB4 y Ethernet directos).",
    how: "En Ajustes del Sistema → Privacidad y seguridad → Red local (System Settings → Privacy & Security → Local Network), activa Tandem y tandemd. Si no aparecen todavía, macOS lo preguntará la primera vez que busquen la PC.",
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
