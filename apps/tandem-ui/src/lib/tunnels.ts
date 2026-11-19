// La pantalla Tunnels (`V3-Compute`) sin DOM: de lo que manda el agente (`tandem_ui::tunnels`)
// a lo que se dibuja, y la lógica de la lista (proyectos plegables, selección, salida en vivo).
// Solo lectura (D9): nada aquí detiene ni abre tunnels.

import type { Machine } from "./model";
import type { OutputChunk, OutputCursor, ProcessRow, TunnelView } from "./store/types";

// ---------------------------------------------------------------------------------------
// Lo que llega del núcleo de la app (`ProjectItem[]`, `OutputItem`, `TransferItem`).

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

/** Los tunnels sin proyecto se agrupan bajo este nombre. */
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
// Qué se acuerda la ventana entre una lista y la siguiente.

/** Lo que la ventana sabe de un tunnel que ya no está en la lista del agente (se cerró). */
export interface Memory {
  /** Los últimos que se vieron, por id. */
  seen: Map<string, TunnelView>;
  /** Cuándo cada tunnel pasó a inactivo (ms), mientras lo siga estando. */
  idle: Map<string, number>;
  /** Bytes copiados por tunnel; cada transferencia cuenta una vez (la clave es la copia). */
  traffic: Map<string, { up: number; down: number }>;
  /** El último avance de cada copia in progress o terminada, para no sumar dos veces. */
  copies: Map<string, number>;
}

export const newMemory = (): Memory => ({ seen: new Map(), idle: new Map(), traffic: new Map(), copies: new Map() });

/**
 * El nombre corto de una machine en el camino de la lista: «Mac», «PC» si son de sistemas distintos.
 * Con el mismo sistema, su primera palabra; si las primeras palabras no las distinguen («PC-Escritorio»
 * y «PC-Taller», «MacBook Pro» y «Mac mini»), lo que sigue; y el nombre completo si tampoco alcanza.
 * Dos nombres distintos nunca dan el mismo rótulo.
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

/** Cuántos tunnels cerrados se muestran como mucho: la lista no crece sin fin. */
export const CLOSED_KEPT = 10;

/** Suma el avance de una copia al tráfico de su tunnel (por la diferencia con el último notice). */
export function addTransfer(m: Memory, t: TransferWire): void {
  const key = `${t.tunnel}|${t.upload ? "up" : "down"}|${t.path}`;
  const before = m.copies.get(key) ?? 0;
  const now = t.done;
  const delta = Math.max(0, now - before);
  // Una copia terminada se olvida: la siguiente con la misma ruta vuelve a contar desde cero.
  if (t.finished) m.copies.delete(key);
  else m.copies.set(key, now);
  const cur = m.traffic.get(t.tunnel) ?? { up: 0, down: 0 };
  m.traffic.set(t.tunnel, t.upload ? { ...cur, up: cur.up + delta } : { ...cur, down: cur.down + delta });
}

/**
 * La lista del agente como la pantalla la dibuja. Lo que ya no está en ella se cerró: se queda
 * como «cerrado» con lo último que se vio de él.
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
// Proyectos y estado.

export interface ProjectGroup {
  name: string;
  path: string | null;
  tunnels: TunnelView[];
  /** Cuántos no están cerrados. */
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

/** «1/2»: tunnels abiertos sobre el total del proyecto. */
export const ratio = (g: ProjectGroup): string => `${g.live}/${g.tunnels.length}`;

/** «4 abiertos · 2 proyectos», o «No open tunnels». */
export function summary(groups: ProjectGroup[]): string {
  const live = groups.reduce((n, g) => n + g.live, 0);
  const projects = groups.filter((g) => g.live > 0).length;
  if (live === 0) return "No open tunnels";
  return `${live} open · ${projects} ${projects === 1 ? "project" : "projects"}`;
}

/** 5400 s → «1 h 30 m»; 34 min → «34 m»; menos de un minuto → «45 s». */
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

/** «activo», «inactivo 9 m», «cerrado». */
export function statusText(t: TunnelView, now: number): string {
  if (t.state === "busy") return "active";
  if (t.state === "closed") return "closed";
  return t.idleSince === null ? "inactive" : `inactive ${elapsed(now - t.idleSince)}`;
}

export const processesLabel = (n: number): string => `${n} ${n === 1 ? "process running" : "processes running"}`;

/** El texto del panel de procesos cuando no hay ninguno. */
export const emptyLabel = (t: TunnelView): string => (t.state === "closed" ? "Tunnel closed" : "Nothing running right now");

/** «0.3 ms», o «—» si no hay camino medido. */
export const latencyLabel = (ms: number | null): string => (ms === null ? "—" : `${ms} ms`);

/** Cuánto tarda cada rayita del tubo en cruzarlo (s): más rápido mientras más activo. */
export function tubeSpeed(state: TunnelView["state"]): { a: number; b: number } | null {
  if (state === "busy") return { a: 0.7, b: 1.2 };
  if (state === "idle") return { a: 3, b: 4 };
  return null;
}

/** El tunnel elegido: el que se pidió si sigue, y si no, el primero que no esté cerrado (o el primero). */
export function pickTunnel(tunnels: TunnelView[], wanted: string | null): TunnelView | null {
  return tunnels.find((t) => t.id === wanted) ?? tunnels.find((t) => t.state !== "closed") ?? tunnels[0] ?? null;
}

/** El proceso elegido, acotado a la lista (la que cambia con cada consulta). */
export function pickProcess(rows: ProcessRow[], wanted: number | null): ProcessRow | null {
  return rows.find((p) => p.id === wanted) ?? rows[0] ?? null;
}

/** Los que corren, en el orden en que se lanzaron (el de sus ids: estable entre consultas). */
export const runningOf = (rows: ProcessRow[]): ProcessRow[] => rows.filter((p) => p.state === "running").sort((a, b) => a.id - b.id);

// ---------------------------------------------------------------------------------------
// La salida en vivo.

/** Lo que se guarda de la salida de un proceso: el final, para que la lista aguante mucho texto. */
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

/** «1 tunnel», «2 tunnels»: el plural no se fija. */
export const countTunnels = (n: number): string => `${n} ${n === 1 ? "tunnel" : "tunnels"}`;

/** Renglón de una machine en la tray: sale de sus datos, sin mirar qué sistema es. */
export function machineLine(m: { id: Machine; power: string; detail: string }, tunnels: readonly { to: Machine; state: string }[]): string {
  if (m.power !== "on") return "off";
  const busy = tunnels.filter((t) => t.to === m.id && t.state === "busy").length;
  return busy > 0 ? countTunnels(busy) : m.detail;
}
