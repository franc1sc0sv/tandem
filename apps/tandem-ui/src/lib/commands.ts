// The commands of the ⌘K / Ctrl+K palette (D15: only Tandem commands, no natural
// language). Pure: which commands exist for a desk and how they are filtered; the palette draws and
// runs them.

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
  /** Gray text on the right (state). */
  detail: string;
  /** Shortcut, or empty. */
  key: string;
  /** Dot color. */
  color: string;
  run: () => unknown;
}

/** What the palette can ask for: the store provides it. */
export interface CommandContext {
  go(route: Route): void;
  applyScene(id: string): Promise<ActResult>;
  setPower(machine: Machine, on: boolean): Promise<ActResult>;
  measureLink(): Promise<ActResult>;
  /** Opens the setup at a step (0 = the first; 1 = pair). */
  openWizard(step?: number): void;
  /** Locks (`on`) or releases the cursor on this machine (T-129). */
  lockHere(on: boolean): Promise<ActResult>;
  can(what: "scenes" | "modules" | "power" | "measure" | "lock"): boolean;
}

const GREY = "#6a6d76";
const TEAL = "#5fbfb0";

/** Short name of a machine for the tunnels ("MacBook → PC-Desktop"). */
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
        title: `Apply “${s.name}”`,
        detail: desk.activeScene === s.id ? "active" : "",
        key: s.key,
        color: s.shows ? color(s.shows) : GREY,
        run: () => ctx.applyScene(s.id),
      });
    }
    out.push({ id: "scene:new", group: "Scenes", title: "New scene…", detail: "", key: "", color: GREY, run: () => ctx.go({ screen: "scene", id: null }) });
  }

  const peer: Machine = desk.here === "a" ? "b" : "a";
  const other = m[peer];
  if (other && ctx.can("power")) {
    const on = other.power === "on";
    out.push({
      id: `power:${peer}`,
      group: "Machines",
      title: `${on ? "Power off" : "Power on"} ${other.name}`,
      detail: on ? "on" : "off",
      key: "",
      color: other.color,
      run: () => ctx.setPower(peer, !on),
    });
  }
  // D1: there is no third machine; the other one can be changed (pair again from scratch).
  out.push({ id: "pair:change", group: "Machines", title: "Change the other machine…", detail: "", key: "", color: TEAL, run: () => ctx.openWizard(1) });

  for (const t of desk.tunnels.filter((t) => t.state !== "closed")) {
    const from = m[t.from];
    const to = m[t.to];
    // The same labels as the Tunnels list ("Mac → PC").
    const [lf, lt] = from && to ? shortLabels(from, to) : [shortName(name(t.from)), shortName(name(t.to))];
    out.push({
      id: `tunnel:${t.id}`,
      group: "Tunnels",
      title: `Go to tunnel “${t.title}”`,
      detail: `${lf} → ${lt} · ${t.state === "busy" ? "active" : "inactive"}`,
      key: "",
      color: color(t.to),
      run: () => ctx.go({ screen: "tunnels" }),
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
    go("monitors", "Monitors", { screen: "monitors" }),
    go("network", "Network", { screen: "network" }),
    go("arrangement", "Arrangement", { screen: "arrangement" }),
    go("settings", "Settings", { screen: "settings" }, mac ? "⌘," : "Ctrl+,"),
    go("desk", "Desk", { screen: "desk" }),
    go("tunnels", "Tunnels", { screen: "tunnels" }),
  );
  for (const id of ["a", "b"] as Machine[]) {
    if (m[id]) out.push(go(`machine:${id}`, m[id].name, { screen: "machine", machine: id }, "", color(id)));
  }

  if (ctx.can("measure")) {
    out.push({ id: "link:measure", group: "Actions", title: "Measure the link", detail: "", key: "", color: GREY, run: () => ctx.measureLink() });
  }
  if (ctx.can("lock")) {
    // Only the command that applies for the state (T-157).
    out.push(
      desk.locked
        ? { id: "lock:off", group: "Actions", title: "Remove the lock", detail: "the cursor is locked here", key: "", color: GREY, run: () => ctx.lockHere(false) }
        : { id: "lock:on", group: "Actions", title: "Lock here", detail: "the cursor does not cross", key: "", color: GREY, run: () => ctx.lockHere(true) },
    );
  }
  out.push({ id: "wizard:open", group: "Actions", title: "Open setup", detail: "", key: "", color: GREY, run: () => ctx.openWizard() });
  return out;
}

/** Lowercase and without accents: "tunnels" finds "Tunnels". */
export function normalize(s: string): string {
  return s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase().trim();
}

export interface CommandGroup {
  name: Group;
  items: Command[];
}

/**
 * The commands that match the search, grouped in the usual order. It matches if
 * all the words of the search are in the title or in the group.
 */
export function filterCommands(cmds: Command[], query: string): CommandGroup[] {
  const words = normalize(query).split(/\s+/).filter(Boolean);
  const hit = (c: Command) => {
    const hay = normalize(`${c.title} ${c.group}`);
    return words.every((w) => hay.includes(w));
  };
  return GROUP_ORDER.map((name) => ({ name, items: cmds.filter((c) => c.group === name && hit(c)) })).filter((g) => g.items.length > 0);
}

/** The visible commands in order, to move with ↑↓. */
export function flatten(groups: CommandGroup[]): Command[] {
  return groups.flatMap((g) => g.items);
}

/** The new index when moving `delta` positions, wrapping around. */
export function move(index: number, delta: number, length: number): number {
  if (length === 0) return 0;
  return (((index + delta) % length) + length) % length;
}
