// Los comandos de la paleta ⌘K / Ctrl+K (D15: solo comandos de Tandem, sin lenguaje
// natural). Puro: qué comandos hay para un puesto y cómo se filtran; la paleta los dibuja y
// ejecuta.

import type { Machine } from "./model";
import type { Route } from "./routes";
import type { ActResult, DeskView } from "./store/types";

export type Group = "Escenas" | "Máquinas" | "Túneles" | "Ir a" | "Acciones";

export const GROUP_ORDER: Group[] = ["Escenas", "Máquinas", "Túneles", "Ir a", "Acciones"];

export interface Command {
  id: string;
  group: Group;
  title: string;
  /** Texto gris a la derecha (estado). */
  detail: string;
  /** Atajo, o vacío. */
  key: string;
  /** Color del punto. */
  color: string;
  run: () => unknown;
}

/** Lo que la paleta puede pedir: lo da el store. */
export interface CommandContext {
  go(route: Route): void;
  applyScene(id: string): Promise<ActResult>;
  setPower(machine: Machine, on: boolean): Promise<ActResult>;
  measureLink(): Promise<ActResult>;
  /** Abre el asistente en un paso (0 = el primero; 1 = emparejar). */
  openWizard(step?: number): void;
  can(what: "scenes" | "modules" | "power" | "measure"): boolean;
}

const GREY = "#6a6d76";
const TEAL = "#5fbfb0";

/** Nombre corto de una máquina para los túneles («MacBook → PC-Escritorio»). */
export function shortName(name: string): string {
  return name.split(" ")[0] ?? name;
}

export function buildCommands(desk: DeskView, ctx: CommandContext, mac: boolean): Command[] {
  const out: Command[] = [];
  const m = desk.machines;
  const color = (id: Machine) => m[id]?.color ?? GREY;
  const name = (id: Machine) => m[id]?.name ?? `máquina ${id.toUpperCase()}`;

  if (ctx.can("scenes")) {
    for (const s of desk.scenes) {
      out.push({
        id: `scene:${s.id}`,
        group: "Escenas",
        title: `Aplicar «${s.name}»`,
        detail: desk.activeScene === s.id ? "activa" : "",
        key: s.key,
        color: s.shows ? color(s.shows) : GREY,
        run: () => ctx.applyScene(s.id),
      });
    }
    out.push({ id: "scene:new", group: "Escenas", title: "Nueva escena…", detail: "", key: "", color: GREY, run: () => ctx.go({ screen: "escena", id: null }) });
  }

  const peer: Machine = desk.here === "a" ? "b" : "a";
  const other = m[peer];
  if (other && ctx.can("power")) {
    const on = other.power === "on";
    out.push({
      id: `power:${peer}`,
      group: "Máquinas",
      title: `${on ? "Apagar" : "Encender"} ${other.name}`,
      detail: on ? "encendida" : "apagada",
      key: "",
      color: other.color,
      run: () => ctx.setPower(peer, !on),
    });
  }
  // D1: no hay tercera máquina; sí se puede cambiar la otra (emparejar de nuevo).
  out.push({ id: "pair:change", group: "Máquinas", title: "Cambiar la otra máquina…", detail: "", key: "", color: TEAL, run: () => ctx.openWizard(1) });

  for (const t of desk.tunnels.filter((t) => t.state !== "closed")) {
    out.push({
      id: `tunnel:${t.id}`,
      group: "Túneles",
      title: `Ir al túnel «${t.title}»`,
      detail: `${shortName(name(t.from))} → ${shortName(name(t.to))} · ${t.state === "busy" ? "activo" : "inactivo"}`,
      key: "",
      color: color(t.to),
      run: () => ctx.go({ screen: "tuneles" }),
    });
  }

  const go = (id: string, title: string, route: Route, key = "", dot = GREY): Command => ({
    id: `go:${id}`,
    group: "Ir a",
    title,
    detail: "",
    key,
    color: dot,
    run: () => ctx.go(route),
  });
  out.push(
    go("monitores", "Monitores", { screen: "monitores" }),
    go("red", "Red", { screen: "red" }),
    go("acomodo", "Acomodo", { screen: "acomodo" }),
    go("ajustes", "Ajustes", { screen: "ajustes" }, mac ? "⌘," : "Ctrl+,"),
    go("puesto", "Puesto", { screen: "puesto" }),
    go("tuneles", "Túneles", { screen: "tuneles" }),
  );
  for (const id of ["a", "b"] as Machine[]) {
    if (m[id]) out.push(go(`maquina:${id}`, m[id].name, { screen: "maquina", machine: id }, "", color(id)));
  }

  if (ctx.can("measure")) {
    out.push({ id: "link:measure", group: "Acciones", title: "Medir el enlace", detail: "", key: "", color: GREY, run: () => ctx.measureLink() });
  }
  out.push({ id: "wizard:open", group: "Acciones", title: "Abrir el asistente", detail: "", key: "", color: GREY, run: () => ctx.openWizard() });
  return out;
}

/** Minúsculas y sin acentos: «tuneles» encuentra «Túneles». */
export function normalize(s: string): string {
  return s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase().trim();
}

export interface CommandGroup {
  name: Group;
  items: Command[];
}

/**
 * Los comandos que coinciden con la búsqueda, agrupados en el orden de siempre. Coincide si
 * todas las palabras de la búsqueda están en el título o en el grupo.
 */
export function filterCommands(cmds: Command[], query: string): CommandGroup[] {
  const words = normalize(query).split(/\s+/).filter(Boolean);
  const hit = (c: Command) => {
    const hay = normalize(`${c.title} ${c.group}`);
    return words.every((w) => hay.includes(w));
  };
  return GROUP_ORDER.map((name) => ({ name, items: cmds.filter((c) => c.group === name && hit(c)) })).filter((g) => g.items.length > 0);
}

/** Los comandos visibles en orden, para moverse con ↑↓. */
export function flatten(groups: CommandGroup[]): Command[] {
  return groups.flatMap((g) => g.items);
}

/** El índice nuevo al moverse `delta` posiciones, dando la vuelta. */
export function move(index: number, delta: number, length: number): number {
  if (length === 0) return 0;
  return (((index + delta) % length) + length) % length;
}
