// Las pestañas de Ajustes (`F-Settings`) y cómo se recorren con el teclado. Puro.

import type { IconName } from "../ui/icons";

export type SettingsTab = "puesto" | "atajos" | "teclado" | "upd" | "avanzado";

export const SETTINGS_TABS: { id: SettingsTab; name: string; icon: IconName }[] = [
  { id: "puesto", name: "Desk and machines", icon: "puesto" },
  { id: "atajos", name: "Shortcuts", icon: "keyboard" },
  { id: "teclado", name: "Keyboard and mouse", icon: "mouse" },
  { id: "upd", name: "Updates", icon: "download" },
  { id: "avanzado", name: "Advanced", icon: "sliders" },
];

/** La pestaña a la que llevan las flechas, Inicio y Fin (lista vertical, sin dar la vuelta). */
export function nextTab(current: SettingsTab, key: string): SettingsTab | null {
  const i = SETTINGS_TABS.findIndex((t) => t.id === current);
  const to = key === "ArrowDown" ? i + 1 : key === "ArrowUp" ? i - 1 : key === "Home" ? 0 : key === "End" ? SETTINGS_TABS.length - 1 : i;
  const next = SETTINGS_TABS[Math.min(Math.max(to, 0), SETTINGS_TABS.length - 1)];
  return next && next.id !== current && ["ArrowDown", "ArrowUp", "Home", "End"].includes(key) ? next.id : null;
}
