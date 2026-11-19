// Las pantallas de la ventana principal y su dirección (`#/tunnels`, `#/machine/b`). Puro.

import type { Machine } from "./model";
import type { Topic } from "./store/types";

export type Screen = "desk" | "tunnels" | "monitors" | "scene" | "progress" | "arrangement" | "crossing" | "network" | "machine" | "settings" | "states";

export type Route = { screen: Exclude<Screen, "machine" | "scene"> } | { screen: "machine"; machine: Machine } | { screen: "scene"; id: string | null };

export const HOME: Route = { screen: "desk" };

export const SCREEN_TITLE: Record<Screen, string> = {
  desk: "Desk",
  tunnels: "Tunnels",
  monitors: "Monitors",
  scene: "Scene",
  progress: "Changes in progress",
  arrangement: "Arrangement",
  crossing: "Crossing test",
  network: "Network",
  machine: "Machine",
  settings: "Settings",
  states: "Trouble states",
};

/** Qué temas necesita en vivo cada pantalla: el store solo se suscribe a esos. */
export const SCREEN_TOPICS: Record<Screen, Topic[]> = {
  desk: ["machines", "resources", "link", "scenes"],
  tunnels: ["machines", "tunnels"],
  monitors: ["machines", "monitors", "scenes"],
  scene: ["machines", "monitors", "scenes"],
  progress: ["machines", "monitors", "network"],
  arrangement: ["machines", "monitors"],
  crossing: ["machines"],
  network: ["machines", "link", "network"],
  machine: ["machines", "resources", "tunnels"],
  settings: ["machines"],
  states: ["machines", "link"],
};

const SIMPLE: Exclude<Screen, "machine" | "scene">[] = ["desk", "tunnels", "monitors", "progress", "arrangement", "network", "settings", "states"];

export function parseHash(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean);
  const [head, arg] = parts;
  if (head === "machine" && (arg === "a" || arg === "b")) return { screen: "machine", machine: arg };
  // El editor de scenes cuelga de Monitors (`#/monitors/scene`, o `…/scene/<id>` para editar).
  if (head === "monitors" && arg === "scene") return { screen: "scene", id: parts[2] ? decodeURIComponent(parts[2]) : null };
  // La prueba de crossing cuelga del arrangement (`#/arrangement/crossing`).
  if (head === "arrangement" && arg === "crossing") return { screen: "crossing" };
  const simple = SIMPLE.find((s) => s === head);
  return simple ? { screen: simple } : HOME;
}

export function formatHash(r: Route): string {
  if (r.screen === "machine") return `#/machine/${r.machine}`;
  if (r.screen === "scene") return r.id === null ? "#/monitors/scene" : `#/monitors/scene/${encodeURIComponent(r.id)}`;
  return r.screen === "crossing" ? "#/arrangement/crossing" : `#/${r.screen}`;
}

export function sameRoute(a: Route, b: Route): boolean {
  return formatHash(a) === formatHash(b);
}
