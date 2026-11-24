// The monitors of the Monitors screen (`V3-Monitors`): from the agent's JSON (`MonitorView`)
// to what is drawn. Pure. D11: a monitor that does not speak the standard is shown "not
// compatible" with its reason and is never asked for anything.

import { wallOf } from "./arrangement";
import { MACHINES } from "./machines";
import type { CableJson, LinkJson, Machine, MonitorCompatJson, MonitorEventJson, MonitorInput, MonitorJson } from "./model";
import type { LinkUi, MonitorEventView, MonitorUi } from "./store/types";

export const INPUT_LABEL: Record<MonitorInput, string> = { display_port: "DP", hdmi: "HDMI" };

/** How many events are listed on the card (the most recent). */
export const EVENTS_SHOWN = 3;

/** A picture's physical size, in millimetres (width, height), as the EDID gives it. */
export type SizeMm = [number, number];

/** The diagonal in inches of a picture `size` mm; `null` without a usable size. */
export function diagonalInches(size: SizeMm | null | undefined): number | null {
  if (!size) return null;
  const [w, h] = size;
  if (!(w > 0 && h > 0)) return null;
  return Math.hypot(w, h) / 25.4;
}

/**
 * The short name of a monitor for sentences and labels: `34"` from its EDID size (T-356, a
 * 797 × 334 mm picture), else from a name that says it (`Odyssey G5 34"`), else the name.
 */
export function shortMonitor(name: string, size?: SizeMm | null): string {
  const d = diagonalInches(size);
  if (d !== null) return `${Math.round(d)}"`;
  return /\d+(?:[.,]\d+)?"/.exec(name)?.[0] ?? name;
}

/**
 * The stand a monitor is drawn with (T-356): each known model family has its own shape and
 * height, the rest a generic one. Known from the EDID model name and size.
 * - `ultrawide-g5`: the 34" Odyssey G5 (LC34G55T, 21:9): a low, wide V-shaped base.
 * - `g5`: a 16:9 Odyssey G5 (the 32" `LS32CG55*`): a taller neck on a narrow trapezoid base.
 */
export type StandKind = "generic" | "ultrawide-g5" | "g5";

export function standOf(name: string, size?: SizeMm | null): StandKind {
  const n = name.toUpperCase();
  const wide = size ? size[0] / size[1] > 2 : false;
  if (/C34G55/.test(n) || (/ODYSSEY G5/.test(n) && wide)) return "ultrawide-g5";
  if (/ODYSSEY G5|S\d\dCG55|C\d\dG55/.test(n)) return "g5";
  return "generic";
}

// The two default accents have the exact tone of the mockup (indicator border); the
// rest is darkened the same way.
const ACCENT: Record<string, string> = { "#7aa7e6": "#4f7cbc", "#a99be8": "#6a5fa6" };

/** The border of the sliding indicator when a machine is chosen. */
export function accentOf(color: string): string {
  const known = ACCENT[color.toLowerCase()];
  if (known) return known;
  const m = /^#([0-9a-f]{6})$/i.exec(color);
  if (!m) return "#3a3d45";
  const n = parseInt(m[1], 16);
  const ch = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => Math.round(v * 0.7).toString(16).padStart(2, "0"));
  return `#${ch.join("")}`;
}

/** `HH:MM` in local time (that of the owning machine's clock, already converted by the agent). */
export function formatClock(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** The icon and text of a firmware event. */
export function eventView(e: MonitorEventJson): MonitorEventView {
  const to = e.input ? INPUT_LABEL[e.input] : null;
  const base = { at: e.at_ms };
  switch (e.kind) {
    case "SwitchRequested":
      return { ...base, kind: "switch", text: to ? `Input → ${to}, requested by Tandem` : "Input change requested by Tandem" };
    case "Input":
      return { ...base, kind: "button", text: to ? `Input → ${to}, with the buttons` : "Input changed with the buttons" };
    case "SwitchDone":
      return { ...base, kind: "switch", text: to ? `Switch to ${to} confirmed` : "Input change confirmed" };
    case "SwitchFailed":
      return { ...base, kind: "failed", text: to ? `Switch to ${to} with no picture` : "Input change with no picture" };
    case "Signal":
      return { ...base, kind: "signal", text: to ? `${to} signal detected` : "Signal detected" };
    case "Boot":
      return { ...base, kind: "boot", text: "The monitor started up" };
    case "Power":
      return { ...base, kind: "power", text: "Power state changed" };
    case "Setting":
      return { ...base, kind: "other", text: "A menu setting changed" };
    case "Lost":
      return { ...base, kind: "other", text: "Events were lost" };
    case "Other":
      return { ...base, kind: "other", text: "Monitor event" };
  }
}

/** The reason shown for a monitor no machine has a DDC channel to (T-340). */
export const NO_CHANNEL = "Seen by its EDID only: no machine has a DDC channel to it now";

function compatOf(c: MonitorCompatJson): MonitorUi["compat"] {
  if (typeof c !== "object") return { ok: false, reason: NO_CHANNEL };
  return "Compatible" in c ? { ok: true } : { ok: false, reason: c.NotCompatible.reason };
}

/** The agent's derived link (T-349) for the UI. */
export function linkUi(j: LinkJson): LinkUi {
  const base = { power: j.power, cables: j.cables };
  if (typeof j.state === "object") {
    if ("shows_machine" in j.state) return { ...base, kind: "shows", machine: j.state.shows_machine };
    return { ...base, kind: "unplugged", machine: j.state.active_input_unplugged };
  }
  return { ...base, kind: j.state, machine: null };
}

/** The monitor shows neither machine: off, in standby or on an unplugged input. */
export function isDim(link: LinkUi | undefined): boolean {
  return link?.kind === "off" || link?.kind === "standby" || link?.kind === "unplugged";
}

/** The plain words of a monitor's link: "Showing PC", "Mac cable unplugged", "Monitor in standby", "Monitor off"; `null` if unknown. */
export function linkText(link: LinkUi | undefined, name: (m: Machine) => string): string | null {
  switch (link?.kind) {
    case "shows":
      return link.machine ? `Showing ${name(link.machine)}` : null;
    case "unplugged":
      return link.machine ? `${name(link.machine)} cable unplugged` : null;
    case "standby":
      return "Monitor in standby";
    case "off":
      return "Monitor off";
    default:
      return null;
  }
}

export function monitorUi(m: MonitorJson): MonitorUi {
  const compat = compatOf(m.compat);
  const ports: MonitorUi["ports"] = {};
  for (const [who, input] of m.via) ports[who] = input;
  const input = m.report?.input ?? null;
  const link = m.link ? linkUi(m.link) : undefined;
  // With the agent's link, what it shows is its derivation (an off monitor shows nobody).
  const shows: Machine | null = link ? (link.kind === "shows" ? link.machine : null) : input ? (MACHINES.find((who) => ports[who] === input) ?? null) : null;
  const events = [...(m.report?.events ?? [])].reverse().slice(0, EVENTS_SHOWN).map(eventView);
  const ui: MonitorUi = { id: m.id, name: m.name, compat, connected: m.seen_by.length > 0, owner: m.owner, input, ports, shows, events };
  if (link) ui.link = link;
  if (m.size_mm && m.size_mm[0] > 0 && m.size_mm[1] > 0) ui.sizeMm = [m.size_mm[0], m.size_mm[1]];
  return ui;
}

/**
 * The screens of each machine that are a monitor showing the other machine (T-111, T-340): the
 * screen of each cable end, by the real input the owner read and which machine that input goes
 * to (the cable's declared one, otherwise the EDID's). Live desktops draws a monitor once, on the
 * machine it shows, as the agent's geometry does.
 */
export function hiddenScreens(cables: CableJson[] | undefined, monitors: MonitorUi[]): Record<Machine, string[]> {
  return deskScreens(cables, monitors, () => "").hidden;
}

/**
 * Like [`hiddenScreens`], with the monitors that show neither machine (T-349): such a monitor is
 * drawn once, dimmed, with its state text (`dim`), on the first machine whose cable names its
 * screen, and is left out of the other.
 */
export function deskScreens(
  cables: CableJson[] | undefined,
  monitors: MonitorUi[],
  name: (m: Machine) => string,
): { hidden: Record<Machine, string[]>; dim: Record<Machine, Record<string, string>> } {
  const hidden: Record<Machine, string[]> = { a: [], b: [] };
  const dim: Record<Machine, Record<string, string>> = { a: {}, b: {} };
  for (const c of cables ?? []) {
    const m = monitors.find((x) => x.id === c.monitor);
    if (m?.link) {
      if (isDim(m.link)) {
        const keep = MACHINES.find((who) => c[who]?.screen);
        if (!keep) continue;
        const other: Machine = keep === "a" ? "b" : "a";
        dim[keep][c[keep]?.screen as string] = linkText(m.link, name) ?? "";
        const screen = c[other]?.screen;
        if (screen) hidden[other].push(screen);
        continue;
      }
      if (m.link.kind === "shows" && m.link.machine) {
        const other: Machine = m.link.machine === "a" ? "b" : "a";
        const screen = c[other]?.screen;
        if (screen) hidden[other].push(screen);
      }
      continue;
    }
    if (!m?.compat.ok || !m.input) continue;
    const shows = MACHINES.find((who) => (c[who]?.input ?? m.ports[who]) === m.input);
    if (!shows) continue;
    const other: Machine = shows === "a" ? "b" : "a";
    const screen = c[other]?.screen;
    if (screen) hidden[other].push(screen);
  }
  return { hidden, dim };
}

/** Compatible first, as the screen shows them; the agent's order is kept within each group. */
export function monitorsUi(list: MonitorJson[]): MonitorUi[] {
  const all = list.map(monitorUi);
  return [...all.filter((m) => m.compat.ok), ...all.filter((m) => !m.compat.ok)];
}

/** Why the change cannot be requested for `who` (or `null` if it can). */
export function whyNotSwitch(m: MonitorUi, who: Machine, name: string): string | null {
  if (!m.compat.ok) return `${m.name} does not support the Tandem standard`;
  if (!m.connected) return `${m.name} is disconnected`;
  // T-349: it would leave a black screen; the agent refuses it too, before any DDC write.
  if (m.link?.cables[who] === "disconnected") return `${name} cable unplugged: switching to it would leave a black screen`;
  if (!m.ports[who]) return `It is not known which input ${name} uses`;
  return null;
}

/** The background of a scene preview (`F-NewScene`): slightly lighter than the thumbnail's. */
export function previewWall(color: string): string {
  const known: Record<string, [string, string]> = { "#7aa7e6": ["#34496a", "#222d3d"], "#a99be8": ["#463d63", "#2a2539"] };
  const hit = known[color.toLowerCase()];
  if (hit) return `linear-gradient(160deg,${hit[0]},${hit[1]})`;
  return wallOf(color);
}

/** Background of "unchanged": the strip of screens the scene does not touch. */
export const KEEP_WALL = "repeating-linear-gradient(45deg,#1b1c21 0 8px,#15161a 8px 16px)";
