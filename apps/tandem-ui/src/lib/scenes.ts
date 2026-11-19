// Scenes (D6): from the agent's desk to the Monitors cards, and the editor draft
// (`F-NewScene`). Puro.

import { shortMonitor } from "./monitors";
import type { Machine, SceneJson } from "./model";
import { formatAccel, isKey } from "./shortcuts";
import type { MonitorUi, SceneView } from "./store/types";

const CTRL = /^Control(Left|Right)?$/;
const ALT = /^Alt(Left|Right)?$/;
const SHIFT = /^Shift(Left|Right)?$/;
const META = /^(Meta|OS)(Left|Right)?$/;

/** `["ControlLeft","AltLeft","Digit1"]` → `Ctrl+Alt+Digit1` (the `formatAccel` format). */
export function accelOfCodes(codes: string[]): string {
  const mods = [codes.some((c) => CTRL.test(c)) && "Ctrl", codes.some((c) => ALT.test(c)) && "Alt", codes.some((c) => SHIFT.test(c)) && "Shift", codes.some((c) => META.test(c)) && "Super"].filter(Boolean);
  const key = codes.find((c) => isKey(c));
  return key ? [...mods, key].join("+") : "";
}

/** `Ctrl+Alt+Digit4` → the codes a scene stores. */
export function codesOfAccel(accel: string): string[] {
  const names: Record<string, string> = { Ctrl: "ControlLeft", Alt: "AltLeft", Shift: "ShiftLeft", Super: "MetaLeft" };
  return accel.split("+").map((p) => names[p] ?? p);
}

/** How a scene's shortcut looks on this machine (`⌃⌥1` or `Ctrl+Alt+1`); empty without a shortcut. */
export function shortcutText(codes: string[], mac: boolean): string {
  return formatAccel(accelOfCodes(codes), mac);
}

/** The same shortcut (with either of the keys of each modifier). */
export function sameShortcut(a: string[], b: string[]): boolean {
  const x = accelOfCodes(a);
  return x !== "" && x === accelOfCodes(b);
}

export interface SceneContext {
  monitors: MonitorUi[];
  name: (m: Machine) => string;
  mac: boolean;
}

/** A scene card: `34" → PC-Desktop`, its shortcut and the machine of the thumbnail. */
export function sceneView(s: SceneJson, ctx: SceneContext): SceneView {
  const known = s.monitors.flatMap(([id, machine]) => {
    const m = ctx.monitors.find((x) => x.id === id);
    return m ? [{ m, machine }] : [];
  });
  const desc = known.length > 0 ? known.map(({ m, machine }) => `${shortMonitor(m.name)} → ${ctx.name(machine)}`).join(", ") : "No monitors changed";
  return {
    id: s.id,
    name: s.name,
    key: shortcutText(s.shortcut, ctx.mac),
    desc,
    shows: s.monitors[0]?.[1] ?? null,
    monitors: s.monitors.map(([monitor, machine]) => ({ monitor, machine })),
    shortcut: s.shortcut,
    raw: s,
  };
}

// ---- The editor ----------------------------------------------------------------------------

/** What a monitor shows in the scene: a machine, or `keep` (unchanged). */
export type Pick = Machine | "keep";

export interface Draft {
  /** `null`: a new scene. */
  id: string | null;
  name: string;
  picks: Record<string, Pick>;
  shortcut: string[];
  /** What the screen does not edit (arrangement, required machine, icon) is preserved. */
  base: SceneJson | null;
}

/** Same as the core (`MAX_SCENE_NAME`). */
export const MAX_NAME = 40;

/** Only compatible ones enter a scene (D11, DESIGN §5). */
export function compatible(monitors: MonitorUi[]): MonitorUi[] {
  return monitors.filter((m) => m.compat.ok);
}

/** Draft of a new scene: each compatible monitor, unchanged. */
export function newDraft(monitors: MonitorUi[]): Draft {
  return { id: null, name: "", picks: Object.fromEntries(compatible(monitors).map((m) => [m.id, "keep" as Pick])), shortcut: [], base: null };
}

export function draftOf(s: SceneView, monitors: MonitorUi[]): Draft {
  const picks: Record<string, Pick> = Object.fromEntries(compatible(monitors).map((m) => [m.id, "keep" as Pick]));
  // A scene monitor that is not visible today is preserved: editing the name does not delete it.
  for (const { monitor, machine } of s.monitors) picks[monitor] = machine;
  return { id: s.id, name: s.name, picks, shortcut: s.shortcut, base: s.raw };
}

export type Problem = { field: "name" | "shortcut"; text: string };

/** What prevents saving; `null` if it can be saved. `others` are the other scenes (names and shortcuts already taken). */
export function problemOf(d: Draft, others: SceneView[]): Problem | null {
  const name = d.name.trim();
  if (name === "") return { field: "name", text: "Give the scene a name" };
  if ([...name].length > MAX_NAME) return { field: "name", text: `The name cannot exceed ${MAX_NAME} characters` };
  const clash = others.find((o) => sameShortcut(o.shortcut, d.shortcut));
  if (clash) return { field: "shortcut", text: `That shortcut already belongs to “${clash.name}”` };
  return null;
}

/** The scene that is saved: what was edited on top of what it already had. A new one goes without an id: the agent gives it one. */
export function sceneOf(d: Draft): SceneJson {
  const monitors = Object.entries(d.picks).flatMap(([monitor, p]) => (p === "keep" ? [] : [[monitor, p] as [string, Machine]]));
  return { id: d.id ?? "", name: d.name.trim(), icon: d.base?.icon, monitors, layout: d.base?.layout ?? null, require_on: d.base?.require_on ?? null, shortcut: d.shortcut };
}
