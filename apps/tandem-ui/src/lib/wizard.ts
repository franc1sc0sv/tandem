// The 6-step setup (DESIGN §12, mockups `F-Setup1`…`F-Setup6`): what does not depend on the
// DOM. Which steps there are, what "What it found" shows, the permission cards, the drawing and the
// verdict of the crossing test and the final summary. Every datum comes from the real state of the
// desk: nothing is assumed by system (D2).

import { formatMs, type CrossPhase } from "./crossing";
import { LINK_NAME } from "./machines";
import type { AdminView, Machine, Permission, Side } from "./model";
import { LOCAL_NETWORK_PATH } from "./permissions";
import type { DeskView, MachineView, PermissionRow } from "./store/types";

export const WIZARD_STEPS = ["This machine", "Your desk", "Permissions", "Keyboard and mouse", "Arrangement and crossing test", "Done"];

/** Steps that are opened from other parts of the app (indices of `WIZARD_STEPS`). */
export const STEP_MACHINE = 0;
export const STEP_PAIR = 1;
export const STEP_PERMISSIONS = 2;
export const STEP_KEYBOARD = 3;
export const STEP_ARRANGE = 4;
export const STEP_DONE = 5;

/** The command that adds Tandem to Claude Code (`docs/dev/mcp.md`). */
export const MCP_COMMAND = "claude mcp add tandem -- tandem mcp";

export type StepMark = "done" | "cur" | "todo";

/** Done (green), current (highlighted) or to do. */
export function stepMark(index: number, current: number): StepMark {
  return index < current ? "done" : index === current ? "cur" : "todo";
}

/** A step further on (or back) without leaving the list. */
export function clampStep(i: number): number {
  return Math.max(0, Math.min(WIZARD_STEPS.length - 1, i));
}

// ---- Step 1: what it found ------------------------------------------------------------------

export interface FoundRow {
  id: string;
  /** `ok` found, `off` unavailable now, `wait` checking. */
  state: "ok" | "off" | "wait";
  label: string;
  note: string;
  /** The note goes in monospace (a list of tools). */
  mono: boolean;
  /** Only the service row: there is an "Install" button. */
  install?: boolean;
}

/** What each active module says about itself, in few words (like the mockup). */
const SHORT: Record<string, string> = { input: "capture and injection", clipboard: "text, HTML, image", monitors: "DDC channel" };
const LABEL: Record<string, string> = { tunnels: "Compute" };
const ORDER = ["input", "clipboard", "tunnels", "monitors"];

/** The environments with the tools of the first that announces them: "zsh · Docker · uv · node". */
export function computeNote(m: MachineView): string {
  const names = m.environments.map((e) => e.name);
  const tools = (m.environments.find((e) => e.kind === "shell")?.detail ?? "")
    .split(" · ")
    .filter((t) => t && !names.some((n) => n.toLowerCase() === t.toLowerCase()));
  return [...names, ...tools].slice(0, 4).join(" · ");
}

const GIB = 1024 ** 3;

/** "macOS 26 · Apple M5 · 24 GB": the system and the hardware according to the scan (D10). */
export function systemNote(m: MachineView): string {
  const hw = m.hardware;
  const parts = [m.system, hw?.cpu, hw && hw.memory > 0 ? `${Math.round(hw.memory / GIB)} GB` : null].filter((p): p is string => !!p);
  return parts.length > 1 ? parts.join(" · ") : m.detail;
}

/**
 * The administrator service row according to what the app reports (`off` / `pending` / `ok`).
 * Without a reading yet the agent's module is used, but it never keeps spinning: without data, `off`.
 */
export function serviceRow(admin: AdminView | null, moduleActive: boolean): FoundRow {
  const base = { id: "service", label: "Administrator service", mono: false };
  if (!admin) return { ...base, state: moduleActive ? "ok" : "off", note: "" };
  const state = admin.state === "ok" ? "ok" : admin.state === "pending" ? "wait" : "off";
  return { ...base, state, note: admin.canInstall ? "" : admin.note, install: admin.canInstall && admin.state !== "ok" };
}

export function foundRows(m: MachineView | null, admin: AdminView | null = null): FoundRow[] {
  if (!m) return [];
  const rows: FoundRow[] = [{ id: "system", state: "ok", label: systemNote(m), note: "system", mono: false }];
  for (const id of ORDER) {
    const mod = m.modules.find((x) => x.id === id);
    if (!mod) continue;
    const label = LABEL[id] ?? mod.name;
    // Compute is drawn with the environments that exist, even if the administrator service is missing.
    if (id === "tunnels" && (mod.state === "active" || m.environments.length > 0)) rows.push({ id, state: "ok", label, note: computeNote(m), mono: true });
    else if (mod.state === "active") rows.push({ id, state: "ok", label, note: SHORT[id] ?? mod.detail, mono: false });
    else if (id === "monitors" && mod.state === "off") rows.push({ id, state: "off", label, note: "no DDC channel right now; another machine controls them", mono: false });
    else rows.push({ id, state: "off", label, note: mod.detail, mono: false });
  }
  const tunnels = m.modules.find((x) => x.id === "tunnels");
  // The app is the one that knows about the service: the row appears even if the agent does not announce the module.
  if (tunnels || admin) rows.push(serviceRow(admin, tunnels?.state === "active"));
  return rows;
}

// ---- Step 3: permissions --------------------------------------------------------------------

export interface PermCard {
  id: Permission;
  title: string;
  why: string;
  /** Granted and checked by the agent. */
  granted: boolean;
  /** Really missing. What the agent cannot tell yet counts neither as missing nor as granted. */
  missing: boolean;
  /** The line under the title: what holds and what to do. */
  status: string;
  /** Offer "Ask again": Local Network while it is not granted (T-344). macOS has no button for it. */
  askAgain: boolean;
}

// The texts of `F-Setup3`: they address the other machines of the desk without naming any system.
const PERM_TEXT: Record<Permission, { title: string; why: string }> = {
  accessibility: { title: "Accessibility", why: "To type and move the mouse on this machine when you use them from another one." },
  input_monitoring: { title: "Input Monitoring", why: "To carry this machine’s keyboard and mouse to another one when the cursor crosses the edge." },
  local_network: { title: "Local Network", why: "To find the other machines of the desk and talk to them encrypted." },
};

/** The status line of a card. Local Network has its own: the agent probes it, and macOS asks only once (T-344). */
export function permStatus(row: PermissionRow): string {
  if (row.state === "granted") return "Granted · checked just now";
  if (row.id === "local_network") {
    return row.state === "missing"
      ? `Blocked by macOS · turn on Tandem (or tandemd) in ${LOCAL_NETWORK_PATH}`
      : "Not checked yet · press “Ask again” and choose Allow when macOS asks";
  }
  return row.state === "missing" ? "Waiting… it is checked automatically" : "Not checked · macOS asks the first time it is used";
}

export function permCards(rows: PermissionRow[]): PermCard[] {
  return rows.map((r) => ({
    id: r.id,
    ...PERM_TEXT[r.id],
    granted: r.state === "granted",
    missing: r.state === "missing",
    status: permStatus(r),
    askAgain: r.id === "local_network" && r.state !== "granted",
  }));
}

export const missingOf = (cards: PermCard[]): number => cards.filter((c) => c.missing).length;

/** «Missing 2» or «All granted». */
export function permFooter(missing: number): string {
  return missing === 0 ? "All granted" : `Missing ${missing}`;
}

// ---- Step 4: keyboard and mouse -------------------------------------------------------------

export interface DeviceRow {
  name: string;
  via?: string;
}

/** Who already showed activity: a key or a movement received on that machine. */
export type Activity = Record<Machine, boolean>;
export const NO_ACTIVITY: Activity = { a: false, b: false };

export function heard(a: Activity, m: Machine): Activity {
  return a[m] ? a : { ...a, [m]: true };
}

/** Keyboards and mice of a machine: only those the agent announces; what is assumed does not count. */
export function deviceRows(m: MachineView | null): DeviceRow[] {
  return m?.inputDevices ?? [];
}

/** What the agent announces about the hub: without `true` there is no keyboard or mouse to show. */
export function devicesOfHub(hub: boolean | null | undefined): DeviceRow[] {
  return hub === true ? [{ name: "Keyboard and mouse", via: "through the hub" }] : [];
}

/** Compute works: the tunnels module is not off and there is a ready environment. */
export function computeWorks(m: MachineView | null | undefined): boolean {
  if (!m) return false;
  // A module the agent reports as unavailable does not count as remote compute either (T-158).
  const state = m.modules.find((x) => x.id === "tunnels")?.state;
  return state !== "off" && state !== "unavailable" && m.environments.some((e) => e.ready);
}

// ---- Step 5: arrangement and crossing test ------------------------------------------------

/** The drawing boxes, on the 290 px tall canvas of `F-Setup5`: its own, the other and the edge. */
export interface Sketch {
  here: string;
  other: string;
  edge: string;
  /** Cursor path: where it leaves from, where it passes and where it ends, in % of the canvas (x) or px (y). */
  axis: "x" | "y";
  path: [number, number, number];
  /** Where the cursor stays after crossing. */
  done: string;
}

export const SKETCH: Record<Side, Sketch> = {
  right: {
    here: "left:12%;top:90px;width:28%;height:130px",
    other: "left:40%;top:40px;width:46%;height:200px",
    edge: "left:calc(40% - 2px);top:60px;width:4px;height:170px",
    axis: "x",
    path: [18, 52, 58],
    done: "left:58%",
  },
  left: {
    here: "left:60%;top:90px;width:28%;height:130px",
    other: "left:14%;top:40px;width:46%;height:200px",
    edge: "left:calc(60% - 2px);top:60px;width:4px;height:170px",
    axis: "x",
    path: [82, 48, 42],
    done: "left:42%",
  },
  down: {
    here: "left:34%;top:20px;width:32%;height:100px",
    other: "left:20%;top:120px;width:60%;height:150px",
    edge: "left:35%;top:118px;width:30%;height:4px",
    axis: "y",
    path: [60, 130, 150],
    done: "top:150px",
  },
  up: {
    here: "left:34%;top:170px;width:32%;height:100px",
    other: "left:20%;top:20px;width:60%;height:150px",
    edge: "left:35%;top:168px;width:30%;height:4px",
    axis: "y",
    path: [220, 150, 130],
    done: "top:130px",
  },
};

const EDGE_WORD: Record<Side, string> = { right: "right", left: "left", up: "top", down: "bottom" };
const BY_WORD: Record<Side, string> = { right: "the right", left: "the left", up: "the top", down: "the bottom" };

export const edgeWord = (s: Side): string => EDGE_WORD[s];

export interface CrossVerdict {
  tone: "ok" | "warn" | "wait";
  text: string;
}

/**
 * What the user is told according to the test phase. `expected` is the arrangement edge,
 * `here` the name of this machine and `peer` that of the other.
 */
export function crossVerdict(phase: CrossPhase, expected: Side, here: string, peer: string, seconds: number): CrossVerdict {
  switch (phase.kind) {
    case "waiting":
      return { tone: "wait", text: `Waiting for the crossing at the ${EDGE_WORD[expected]} edge of ${here}…` };
    case "passed":
      return { tone: "ok", text: `Crossed to ${peer} through the ${EDGE_WORD[expected]} edge, as the arrangement says.${phase.sample.ms === null ? "" : ` Round trip measured: ${formatMs(phase.sample.ms)}.`}` };
    case "wrong-edge":
      return {
        tone: "warn",
        text: `Crossed at ${BY_WORD[phase.sample.edge]}, not ${BY_WORD[expected]}: the arrangement does not match your desk. Move the screens and test again.`,
      };
    case "timeout":
      return { tone: "warn", text: `Did not cross in ${seconds} s. Move the mouse to the ${EDGE_WORD[expected]} edge of ${here} and test again, or move the screens.` };
    case "unavailable":
      return { tone: "warn", text: `The crossing cannot be tested yet: ${phase.reason}.` };
  }
}

/** Where the other machine ends up according to where it was dropped, seen from the center of its own. */
export function dropSide(dx: number, dy: number): Side {
  return Math.abs(dx) >= Math.abs(dy) ? (dx >= 0 ? "right" : "left") : dy >= 0 ? "down" : "up";
}

/** The keyboard arrow moves the other machine toward that side. */
export function arrowSide(key: string): Side | null {
  return key === "ArrowRight" ? "right" : key === "ArrowLeft" ? "left" : key === "ArrowUp" ? "up" : key === "ArrowDown" ? "down" : null;
}

// ---- Step 6: summary -----------------------------------------------------------------------

export interface SummaryRow {
  id: string;
  label: string;
  note: string;
  mono: boolean;
  /** What was truly verified; `false` if not yet. */
  ok: boolean;
}

export interface Facts {
  /** The step 5 crossing passed with the arrangement saved. */
  crossed: boolean;
}

/** «USB-C · 0.3 ms». */
export function linkNote(d: DeskView): string | null {
  const l = d.link;
  if (!l.kind) return null;
  return l.ms === null ? LINK_NAME[l.kind] : `${LINK_NAME[l.kind]} · ${formatMs(l.ms)}`;
}

export function summaryRows(d: DeskView, facts: Facts): SummaryRow[] {
  const here = d.machines[d.here];
  const peer = d.machines[d.here === "a" ? "b" : "a"];
  const link = linkNote(d);
  const input = here?.modules.find((x) => x.id === "input");
  const mon = d.monitors.find((x) => x.compat.ok && x.connected);
  const tunnels = [here, peer].every(computeWorks);
  return [
    { id: "link", label: "Link", note: link ?? "no link", mono: link !== null, ok: link !== null },
    {
      id: "input",
      label: "Keyboard and mouse",
      note: facts.crossed ? "from any machine · crossing tested" : "crossing not tested",
      mono: false,
      ok: input?.state === "active" && facts.crossed,
    },
    {
      id: "monitors",
      label: "Monitors",
      note: mon ? `${d.layout.screens.find((x) => x.id === mon.id)?.size ?? mon.name} compatible${mon.owner ? ` · owner ${d.machines[mon.owner]?.name ?? ""}` : ""}` : "none supported",
      mono: false,
      ok: !!mon,
    },
    {
      id: "tunnels",
      label: "Remote compute",
      note: tunnels ? "MCP ready for Claude Code on both" : "remote compute does not respond on one of them",
      mono: false,
      ok: tunnels,
    },
  ];
}

/** The first row of the summary: "MacBook Pro and PC-Desktop in "My desk"". */
export function deskTitle(d: DeskView): { here: string; peer: string | null; desk: string } {
  const peerId: Machine = d.here === "a" ? "b" : "a";
  const peer = d.pair.peer.kind === "paired" ? (d.machines[peerId]?.name ?? d.pair.peer.name) : null;
  return { here: d.machines[d.here]?.name ?? "This machine", peer, desk: d.deskName };
}
