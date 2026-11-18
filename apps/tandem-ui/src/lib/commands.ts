// Los comandos de la paleta ⌘K / Ctrl+K (D15: solo comandos de Tandem, sin lenguaje
// natural). Puro: qué comandos hay para un puesto y cómo se filtran; la paleta los dibuja y
// ejecuta.

import type { Machine } from "./model";
import { shortLabels } from "./tunnels";
import type { Route } from "./routes";
import type { ActResult, DeskView } from "./store/types";

export type Group = "Scenes" | "Machines" | "Tunnels" | "Go to" | "Actions";

export const GROUP_ORDER: Group[] = ["Scenes", "Machines", "Tunnels", "Go to", "Actions"];

export interface Command {
  id: string;
  group: Group;
  title: string;
  /** Texto gris to the right of (estado). */
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
  /** Bloquea (`on`) o suelta el cursor en this machine (T-129). */
  lockHere(on: boolean): Promise<ActResult>;
  can(what: "scenes" | "modules" | "power" | "measure" | "lock"): boolean;
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
  const name = (id: Machine) => m[id]?.name ?? `machine ${id.toUpperCase()}`;

  if (ctx.can("scenes")) {
    for (const s of desk.scenes) {
      out.push({
        id: `scene:${s.id}`,
        group: "Scenes",
        title: `Aplicar «${s.name}»`,
        detail: desk.activeScene === s.id ? "activa" : "",
        key: s.key,
        color: s.shows ? color(s.shows) : GREY,
        run: () => ctx.applyScene(s.id),
      });
    }
    out.push({ id: "scene:new", group: "Scenes", title: "New scene…", detail: "", key: "", color: GREY, run: () => ctx.go({ screen: "escena", id: null }) });
  }

  const peer: Machine = desk.here === "a" ? "b" : "a";
  const other = m[peer];
  if (other && ctx.can("power")) {
    const on = other.power === "on";
    out.push({
      id: `power:${peer}`,
      group: "Machines",
      title: `${on ? "Power off" : "Power on"} ${other.name}`,
      detail: on ? "encendida" : "apagada",
      key: "",
      color: other.color,
      run: () => ctx.setPower(peer, !on),
    });
  }
  // D1: no hay tercera máquina; sí se puede cambiar la otra (emparejar de nuevo).
  out.push({ id: "pair:change", group: "Machines", title: "Change the other machine…", detail: "", key: "", color: TEAL, run: () => ctx.openWizard(1) });

  for (const t of desk.tunnels.filter((t) => t.state !== "closed")) {
    const from = m[t.from];
    const to = m[t.to];
    // Los mismos rótulos que la lista de Túneles («Mac → PC»).
    const [lf, lt] = from && to ? shortLabels(from, to) : [shortName(name(t.from)), shortName(name(t.to))];
    out.push({
      id: `tunnel:${t.id}`,
      group: "Tunnels",
      title: `Go to tunnel “${t.title}”`,
      detail: `${lf} → ${lt} · ${t.state === "busy" ? "activo" : "inactivo"}`,
      key: "",
      color: color(t.to),
      run: () => ctx.go({ screen: "tuneles" }),
    });
  }

  const go = (id: string, title: string, route: Route, key = "", dot = GREY): Command => ({
    id: `go:${id}`,
    group: "Go to",
    title,
    detail: "",
    key,
    color: dot,
    run: () => ctx.go(route),
  });
  out.push(
    go("monitores", "Monitors", { screen: "monitores" }),
    go("red", "Network", { screen: "red" }),
    go("acomodo", "Arrangement", { screen: "acomodo" }),
    go("ajustes", "Settings", { screen: "ajustes" }, mac ? "⌘," : "Ctrl+,"),
    go("puesto", "Desk", { screen: "puesto" }),
    go("tuneles", "Tunnels", { screen: "tuneles" }),
  );
  for (const id of ["a", "b"] as Machine[]) {
    if (m[id]) out.push(go(`maquina:${id}`, m[id].name, { screen: "maquina", machine: id }, "", color(id)));
  }

  if (ctx.can("measure")) {
    out.push({ id: "link:measure", group: "Actions", title: "Measure the link", detail: "", key: "", color: GREY, run: () => ctx.measureLink() });
  }
  if (ctx.can("lock")) {
    // Solo la orden que aplica según el estado (T-157).
    out.push(
      desk.locked
        ? { id: "lock:off", group: "Actions", title: "Remove the lock", detail: "the cursor is locked here", key: "", color: GREY, run: () => ctx.lockHere(false) }
        : { id: "lock:on", group: "Actions", title: "Lock here", detail: "the cursor does not cross", key: "", color: GREY, run: () => ctx.lockHere(true) },
    );
  }
  out.push({ id: "wizard:open", group: "Actions", title: "Open setup", detail: "", key: "", color: GREY, run: () => ctx.openWizard() });
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
