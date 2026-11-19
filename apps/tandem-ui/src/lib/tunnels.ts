// The Tunnels screen (`V3-Compute`) without a DOM: from what the agent sends (`tandem_ui::tunnels`)
// to what is drawn, and the list logic (collapsible projects, selection, live output).
// Read-only (D9): nothing here stops or opens tunnels.

import type { Machine } from "./model";
import type { OutputChunk, OutputCursor, ProcessRow, TunnelView } from "./store/types";

// ---------------------------------------------------------------------------------------
// What arrives from the app core (`ProjectItem[]`, `OutputItem`, `TransferItem`).

export interface ProcessWire {
  id: number;
  command: string;
  pid: number | null;
  started: number;
  duration: number | null;
  state: { kind: "running" } | { kind: "exited"; code: number | null } | { kind: "killed" };
}

export interface TunnelWire {
  id: string;
  title: string;
  origin: Machine;
  target: Machine;
  open: boolean;
  created: number;
  running: ProcessWire[];
}

export interface ProjectWire {
  name: string | null;
  root: string | null;
  tunnels: TunnelWire[];
}

export interface OutputWire {
  process: ProcessWire;
  stdout: string;
  stderr: string;
  next_stdout: number;
  next_stderr: number;
  done: boolean;
}

export interface TransferWire {
  tunnel: string;
  upload: boolean;
  path: string;
  done: number;
  total: number;
  finished: boolean;
}

/** Tunnels without a project are grouped under this name. */
export const NO_PROJECT = "No project";

export function processRow(p: ProcessWire): ProcessRow {
  return {
    id: p.id,
    command: p.command,
    pid: p.pid,
    startedAt: p.started,
    state: p.state.kind,
    code: p.state.kind === "exited" ? p.state.code : null,
  };
}

export function outputChunk(o: OutputWire): OutputChunk {
  return { text: o.stdout + o.stderr, next: { stdout: o.next_stdout, stderr: o.next_stderr }, done: o.done };
}

// ---------------------------------------------------------------------------------------
// What the window remembers between one list and the next.

/** What the window knows about a tunnel that is no longer in the agent's list (it closed). */
export interface Memory {
  /** The last ones seen, by id. */
  seen: Map<string, TunnelView>;
  /** When each tunnel became idle (ms), while it still is. */
  idle: Map<string, number>;
  /** Bytes copied per tunnel; each transfer counts once (the key is the copy). */
  traffic: Map<string, { up: number; down: number }>;
  /** The last progress of each copy in progress or finished, to avoid adding twice. */
  copies: Map<string, number>;
}

export const newMemory = (): Memory => ({ seen: new Map(), idle: new Map(), traffic: new Map(), copies: new Map() });

/**
 * The short name of a machine on the list path: "Mac", "PC" if they are different systems.
 * With the same system, its first word; if the first words do not tell them apart ("PC-Desktop"
 * and "PC-Workshop", "MacBook Pro" and "Mac mini"), what follows; and the full name if that is not enough either.
 * Two different names never give the same label.
 */
export function shortLabels(a: { name: string; os: "macos" | "windows" }, b: { name: string; os: "macos" | "windows" }): [string, string] {
  if (a.os !== b.os) return [a.os === "macos" ? "Mac" : "PC", b.os === "macos" ? "Mac" : "PC"];
  const first = (n: string) => n.split(/[\s-]/)[0] || n;
  const rest = (n: string) => n.slice(first(n).length).replace(/^[\s-]+/, "");
  const [fa, fb] = [first(a.name), first(b.name)];
  const [la, lb] = [fa.toLowerCase(), fb.toLowerCase()];
  if (!la.startsWith(lb) && !lb.startsWith(la)) return [fa, fb];
  const [ra, rb] = [rest(a.name), rest(b.name)];
  if (ra && rb && ra !== rb) return [ra, rb];
  return [a.name, b.name];
}

/** How many closed tunnels are shown at most: the list does not grow without end. */
export const CLOSED_KEPT = 10;

/** Adds the progress of a copy to its tunnel's traffic (by the difference from the last notice). */
export function addTransfer(m: Memory, t: TransferWire): void {
  const key = `${t.tunnel}|${t.upload ? "up" : "down"}|${t.path}`;
  const before = m.copies.get(key) ?? 0;
  const now = t.done;
  const delta = Math.max(0, now - before);
  // A finished copy is forgotten: the next one with the same path counts from zero again.
  if (t.finished) m.copies.delete(key);
  else m.copies.set(key, now);
  const cur = m.traffic.get(t.tunnel) ?? { up: 0, down: 0 };
  m.traffic.set(t.tunnel, t.upload ? { ...cur, up: cur.up + delta } : { ...cur, down: cur.down + delta });
}

/**
 * The agent's list as the screen draws it. What is no longer in it has closed: it stays
 * as "closed" with the last thing seen of it.
 */
export function tunnelsFromWire(groups: ProjectWire[], now: number, m: Memory): TunnelView[] {
  const out: TunnelView[] = [];
  const present = new Set<string>();
  for (const g of groups) {
    const project = g.name ?? NO_PROJECT;
    for (const t of g.tunnels) {
      present.add(t.id);
      const busy = t.running.length > 0;
      if (!t.open) continue;
      if (busy) m.idle.delete(t.id);
      else if (!m.idle.has(t.id)) m.idle.set(t.id, m.seen.has(t.id) ? now : t.created);
      const view: TunnelView = {
        id: t.id,
        project,
        projectPath: g.root,
        title: t.title,
        from: t.origin,
        to: t.target,
        state: busy ? "busy" : "idle",
        processes: t.running.length,
        branch: null,
        created: t.created,
        idleSince: busy ? null : (m.idle.get(t.id) ?? null),
        closedAt: null,
        env: null,
        commands: null,
        ports: null,
        traffic: m.traffic.get(t.id) ?? { up: 0, down: 0 },
      };
      m.seen.set(t.id, view);
      out.push(view);
    }
  }
  const gone = [...m.seen.values()].filter((v) => !present.has(v.id) || v.state === "closed");
  for (const v of gone) {
    m.idle.delete(v.id);
    const closed: TunnelView = v.state === "closed" ? v : { ...v, state: "closed", processes: 0, idleSince: null, closedAt: now };
    m.seen.set(v.id, closed);
  }
  const closed = [...m.seen.values()].filter((v) => v.state === "closed").sort((a, b) => (b.closedAt ?? 0) - (a.closedAt ?? 0));
  for (const v of closed.slice(CLOSED_KEPT)) m.seen.delete(v.id);
  for (const v of closed.slice(0, CLOSED_KEPT)) out.push({ ...v, traffic: m.traffic.get(v.id) ?? v.traffic });
  return out;
}

// ---------------------------------------------------------------------------------------
// Projects and state.

export interface ProjectGroup {
  name: string;
  path: string | null;
  tunnels: TunnelView[];
  /** How many are not closed. */
  live: number;
}

export function groupByProject(tunnels: TunnelView[]): ProjectGroup[] {
  const groups: ProjectGroup[] = [];
  for (const t of tunnels) {
    let g = groups.find((x) => x.name === t.project);
    if (!g) groups.push((g = { name: t.project, path: t.projectPath, tunnels: [], live: 0 }));
    g.tunnels.push(t);
    if (t.state !== "closed") g.live += 1;
  }
  return groups;
}

/** "1/2": open tunnels over the project's total. */
export const ratio = (g: ProjectGroup): string => `${g.live}/${g.tunnels.length}`;

/** "4 open · 2 projects", or "No open tunnels". */
export function summary(groups: ProjectGroup[]): string {
  const live = groups.reduce((n, g) => n + g.live, 0);
  const projects = groups.filter((g) => g.live > 0).length;
  if (live === 0) return "No open tunnels";
  return `${live} open · ${projects} ${projects === 1 ? "project" : "projects"}`;
}

/** 5400 s → "1 h 30 m"; 34 min → "34 m"; under a minute → "45 s". */
export function elapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} m`;
  const h = Math.floor(m / 60);
  return m % 60 === 0 ? `${h} h` : `${h} h ${m % 60} m`;
}

/** 120 000 000 → «120 MB», 2 100 000 000 → «2.1 GB». */
export function bytesLabel(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = Math.max(0, n);
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i += 1;
  }
  const r = v < 10 && i > 0 ? Math.round(v * 10) / 10 : Math.round(v);
  return `${r} ${units[i]}`;
}

/** «activo», «inactivo 9 m», «closed». */
export function statusText(t: TunnelView, now: number): string {
  if (t.state === "busy") return "active";
  if (t.state === "closed") return "closed";
  return t.idleSince === null ? "inactive" : `inactive ${elapsed(now - t.idleSince)}`;
}

export const processesLabel = (n: number): string => `${n} ${n === 1 ? "process running" : "processes running"}`;

/** The text of the process panel when there are none. */
export const emptyLabel = (t: TunnelView): string => (t.state === "closed" ? "Tunnel closed" : "Nothing running right now");

/** "0.3 ms", or "—" if there is no measured path. */
export const latencyLabel = (ms: number | null): string => (ms === null ? "—" : `${ms} ms`);

/** How long each dash of the tube takes to cross it (s): faster the more active. */
export function tubeSpeed(state: TunnelView["state"]): { a: number; b: number } | null {
  if (state === "busy") return { a: 0.7, b: 1.2 };
  if (state === "idle") return { a: 3, b: 4 };
  return null;
}

/** The chosen tunnel: the requested one if it is still there, otherwise the first that is not closed (or the first). */
export function pickTunnel(tunnels: TunnelView[], wanted: string | null): TunnelView | null {
  return tunnels.find((t) => t.id === wanted) ?? tunnels.find((t) => t.state !== "closed") ?? tunnels[0] ?? null;
}

/** The chosen process, clamped to the list (which changes with each query). */
export function pickProcess(rows: ProcessRow[], wanted: number | null): ProcessRow | null {
  return rows.find((p) => p.id === wanted) ?? rows[0] ?? null;
}

/** Those that are running, in the order they were launched (that of their ids: stable between queries). */
export const runningOf = (rows: ProcessRow[]): ProcessRow[] => rows.filter((p) => p.state === "running").sort((a, b) => a.id - b.id);

// ---------------------------------------------------------------------------------------
// Live output.

/** What is kept of a process's output: the tail, so the list withstands a lot of text. */
export const OUTPUT_KEEP = 200_000;

export interface Output {
  text: string;
  cursor: OutputCursor;
}

export const EMPTY_OUTPUT: Output = { text: "", cursor: { stdout: 0, stderr: 0 } };

export function appendOutput(o: Output, chunk: OutputChunk): Output {
  const text = o.text + chunk.text;
  return { text: text.length > OUTPUT_KEEP ? text.slice(text.length - OUTPUT_KEEP) : text, cursor: chunk.next };
}

/** "1 tunnel", "2 tunnels": the plural is not fixed. */
export const countTunnels = (n: number): string => `${n} ${n === 1 ? "tunnel" : "tunnels"}`;

/** A machine row in the tray: comes from its data, without looking at which system it is. */
export function machineLine(m: { id: Machine; power: string; detail: string }, tunnels: readonly { to: Machine; state: string }[]): string {
  if (m.power !== "on") return "off";
  const busy = tunnels.filter((t) => t.to === m.id && t.state === "busy").length;
  return busy > 0 ? countTunnels(busy) : m.detail;
}
