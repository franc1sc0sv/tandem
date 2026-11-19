// The monitors of the Monitors screen (`V3-Monitors`): from the agent's JSON (`MonitorView`)
// to what is drawn. Pure. D11: a monitor that does not speak the standard is shown "not
// compatible" with its reason and is never asked for anything.

import { wallOf } from "./arrangement";
import { MACHINES } from "./machines";
import type { Machine, MonitorEventJson, MonitorInput, MonitorJson } from "./model";
import type { MonitorEventView, MonitorUi } from "./store/types";

export const INPUT_LABEL: Record<MonitorInput, string> = { display_port: "DP", hdmi: "HDMI" };

/** How many events are listed on the card (the most recent). */
export const EVENTS_SHOWN = 3;

/** The short name of a monitor for sentences ("34"" from `Odyssey G5 34"`). */
export function shortMonitor(name: string): string {
  return /\d+(?:[.,]\d+)?"/.exec(name)?.[0] ?? name;
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

export function monitorUi(m: MonitorJson): MonitorUi {
  const compat = "Compatible" in m.compat ? ({ ok: true } as const) : ({ ok: false, reason: m.compat.NotCompatible.reason } as const);
  const ports: MonitorUi["ports"] = {};
  for (const [who, input] of m.via) ports[who] = input;
  const input = m.report?.input ?? null;
  const shows: Machine | null = input ? (MACHINES.find((who) => ports[who] === input) ?? null) : null;
  const events = [...(m.report?.events ?? [])].reverse().slice(0, EVENTS_SHOWN).map(eventView);
  return { id: m.id, name: m.name, compat, connected: m.seen_by.length > 0, owner: m.owner, input, ports, shows, events };
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
