// Las pantallas de la ventana principal y su dirección (`#/tuneles`, `#/maquina/b`). Puro.

import type { Machine } from "./model";
import type { Topic } from "./store/types";

export type Screen = "puesto" | "tuneles" | "monitores" | "acomodo" | "cruce" | "red" | "maquina" | "ajustes";

export type Route = { screen: Exclude<Screen, "maquina"> } | { screen: "maquina"; machine: Machine };

export const HOME: Route = { screen: "puesto" };

export const SCREEN_TITLE: Record<Screen, string> = {
  puesto: "Puesto",
  tuneles: "Túneles",
  monitores: "Monitores",
  acomodo: "Acomodo",
  cruce: "Prueba de cruce",
  red: "Red",
  maquina: "Máquina",
  ajustes: "Ajustes",
};

/** Qué temas necesita en vivo cada pantalla: el store solo se suscribe a esos. */
export const SCREEN_TOPICS: Record<Screen, Topic[]> = {
  puesto: ["machines", "resources", "link", "scenes"],
  tuneles: ["machines", "tunnels"],
  monitores: ["machines", "monitors", "scenes"],
  acomodo: ["machines", "monitors"],
  cruce: ["machines"],
  red: ["machines", "link", "network"],
  maquina: ["machines", "resources", "tunnels"],
  ajustes: ["machines"],
};

const SIMPLE: Exclude<Screen, "maquina">[] = ["puesto", "tuneles", "monitores", "acomodo", "red", "ajustes"];

export function parseHash(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean);
  const [head, arg] = parts;
  if (head === "maquina" && (arg === "a" || arg === "b")) return { screen: "maquina", machine: arg };
  // La prueba de cruce cuelga del acomodo (`#/acomodo/cruce`).
  if (head === "acomodo" && arg === "cruce") return { screen: "cruce" };
  const simple = SIMPLE.find((s) => s === head);
  return simple ? { screen: simple } : HOME;
}

export function formatHash(r: Route): string {
  if (r.screen === "maquina") return `#/maquina/${r.machine}`;
  return r.screen === "cruce" ? "#/acomodo/cruce" : `#/${r.screen}`;
}

export function sameRoute(a: Route, b: Route): boolean {
  return formatHash(a) === formatHash(b);
}
