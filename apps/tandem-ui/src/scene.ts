// Lo que la escena muestra a partir del estado del agente. Puro, para probarlo sin DOM.

import type {
  BuiltinDisplay,
  Desk,
  DeskState,
  Input,
  InputView,
  Link,
  Machine,
  Mode,
  MonitorConfig,
  PeerView,
  Step,
  TxnView,
} from "./model";

export const MACHINES: Machine[] = ["mac", "pc"];
export const MACHINE_NAME: Record<Machine, string> = { mac: "Mac", pc: "PC" };
export const PORT: Record<Input, string> = { hdmi: "HDMI", displayport: "DP" };

export const LINK_NAME: Record<Link, string> = { usb4: "USB4", ethernet: "Ethernet", wifi: "Wi-Fi" };

/** La píldora del enlace: con qué se ve al otro agente, o por qué no. */
export function peerPill(peer: PeerView): { text: string; title: string; ok: boolean } {
  if (peer.kind === "unpaired") {
    return { text: "sin par", title: "Sin par emparejado: esta máquina trabaja sola", ok: false };
  }
  if (peer.link === null) {
    return { text: "sin enlace", title: `Ningún enlace ve a ${peer.name}`, ok: false };
  }
  const note = peer.link === "wifi" ? " (solo control: los servicios requieren cable)" : "";
  return { text: LINK_NAME[peer.link], title: `${peer.name} por ${LINK_NAME[peer.link]}${note}`, ok: true };
}

export const PERMISSION_NAME: Record<InputView["missing"][number], string> = {
  accessibility: "Accesibilidad",
  input_monitoring: "Monitorización de entrada",
};

/** El chip de teclado y mouse de una máquina; `null` si no se muestra. */
export interface RoleChip {
  kind: "server" | "client" | "warn" | "idle";
  icon: "keyboard" | "cursor" | "warn";
  text: string;
  title: string;
}

/**
 * El chip de `who` visto desde el agente de `agent`. Con la PC apagada solo la Mac lleva
 * chip, si tiene el teclado. Los permisos que faltan y los problemas son de esta máquina:
 * su chip los avisa antes que nada.
 */
export function roleChip(input: InputView, agent: Machine, who: Machine, pcOn: boolean): RoleChip | null {
  const mine = who === agent;
  if (mine && input.missing.length > 0) {
    const names = input.missing.map((p) => PERMISSION_NAME[p]).join(" y ");
    return {
      kind: "warn",
      icon: "warn",
      text: "faltan permisos",
      title: `Teclado y mouse: falta ${names} (Ajustes del Sistema → Privacidad y seguridad)`,
    };
  }
  if (mine && input.problem) {
    return { kind: "warn", icon: "warn", text: "sin teclado y mouse", title: `Teclado y mouse: ${input.problem}` };
  }
  if (!pcOn) {
    return who === "mac" && input.server === "mac"
      ? { kind: "server", icon: "keyboard", text: "Teclado aquí", title: "El teclado y el mouse están en la Mac" }
      : null;
  }
  const server = input.server === who;
  const base = server
    ? { kind: "server" as const, icon: "keyboard" as const, text: input.locked ? "Servidor · bloqueado" : "Servidor" }
    : { kind: "client" as const, icon: "cursor" as const, text: "Cliente" };
  const titles = [server ? `${MACHINE_NAME[who]} tiene el teclado y el mouse` : `${MACHINE_NAME[who]} recibe el teclado y el mouse`];
  if (server && input.locked) titles.push("el cursor está bloqueado en su pantalla (atajo)");
  if (!input.session) titles.push("sin sesión con el otro agente");
  return { ...base, kind: input.session ? base.kind : "idle", title: titles.join(" · ") };
}

export type ModeKey = "mac" | "shared" | "pc";
export const MODES: ModeKey[] = ["mac", "shared", "pc"];

export function other(m: Machine): Machine {
  return m === "mac" ? "pc" : "mac";
}

export function ownerOf(state: DeskState, id: string): Machine {
  // Un monitor sin dueño guardado está en la Mac: es el estado seguro de partida.
  return state.owner[id] ?? "mac";
}

/** El modo que describe el reparto actual. */
export function modeOf(desk: Desk, state: DeskState): ModeKey | null {
  if (desk.monitors.length === 0) return null;
  const owners = desk.monitors.map((m) => ownerOf(state, m.id));
  if (owners.every((o) => o === "mac")) return "mac";
  if (owners.every((o) => o === "pc")) return "pc";
  return "shared";
}

/**
 * Qué pedir al elegir una tarjeta. Compartido desde un modo «solo» manda el primer monitor
 * (el de la izquierda) a la PC y deja el resto en la Mac; si ya es compartido, lo deja igual.
 */
export function modeRequest(key: ModeKey, desk: Desk, state: DeskState): Mode {
  if (key === "mac") return { mode: "solo_mac" };
  if (key === "pc") return { mode: "solo_pc" };
  if (modeOf(desk, state) === "shared") {
    return { mode: "shared", assign: Object.fromEntries(desk.monitors.map((m) => [m.id, ownerOf(state, m.id)])) };
  }
  return {
    mode: "shared",
    assign: Object.fromEntries(desk.monitors.map((m, i) => [m.id, i === 0 ? "pc" : "mac"] as const)),
  };
}

/** Los ultrawide (34" o más) se dibujan más anchos. */
export function isWide(m: MonitorConfig): boolean {
  const inches = Number.parseFloat(m.label);
  return Number.isFinite(inches) && inches >= 34;
}

export interface MonitorView {
  id: string;
  label: string;
  owner: Machine;
  /** Entrada por la que lo ve su dueño, si está cableada. */
  port: string | null;
  wide: boolean;
  /** Se puede mandar a la otra máquina (tiene cable hacia allá). */
  movable: boolean;
}

export function monitorsOf(desk: Desk, state: DeskState, who: Machine): MonitorView[] {
  return desk.monitors
    .filter((m) => ownerOf(state, m.id) === who)
    .map((m) => {
      const owner = ownerOf(state, m.id);
      const input = m.wiring[owner];
      return {
        id: m.id,
        label: m.label,
        owner,
        port: input ? PORT[input] : null,
        wide: isWide(m),
        movable: m.wiring[other(owner)] !== undefined,
      };
    });
}

export interface BuiltinView {
  effective: BuiltinDisplay;
  /** Sin monitores externos se enciende sola y no se puede elegir otra cosa. */
  forced: boolean;
  note: string;
}

const BUILTIN_NOTE: Record<BuiltinDisplay, string> = {
  off: "apagada",
  mirror: "espejo",
  extend: "extendida",
};

export function builtinOf(desk: Desk, state: DeskState): BuiltinView {
  const forced = monitorsOf(desk, state, "mac").length === 0;
  const effective = forced ? "extend" : state.builtin_pref;
  return { effective, forced, note: forced ? "encendida: no hay monitores" : BUILTIN_NOTE[effective] };
}

// ---------------------------------------------------------------------------------------
// Progreso de una transacción

export type StepStatus = "pending" | "run" | "done" | "failed";

export interface StepGroup {
  label: string;
  /** Índices de los pasos reales que agrupa. */
  steps: number[];
}

function labelOf(desk: Desk, id: string): string {
  return desk.monitors.find((m) => m.id === id)?.label ?? id;
}

const BUILTIN_STEP: Record<BuiltinDisplay, string> = {
  off: "Pantalla Mac apagada",
  mirror: "Pantalla Mac en espejo",
  extend: "Pantalla Mac encendida",
};

/** Clave y texto de cada paso; los pasos seguidos con la misma clave se ven como uno. */
function describeStep(desk: Desk, s: Step): { key: string; label: string } {
  switch (s.step) {
    case "power_on_pc":
      return { key: "power_on", label: "Encender la PC" };
    case "power_off_pc":
      return { key: "power_off", label: "Apagado limpio" };
    case "switch_input":
      return { key: `monitor:${s.monitor}`, label: `${labelOf(desk, s.monitor)} → ${PORT[s.input]}` };
    // Sumar y quitar el monitor de cada escritorio es parte de moverlo.
    case "attach":
    case "detach":
      return { key: `monitor:${s.monitor}`, label: `${labelOf(desk, s.monitor)} en ${MACHINE_NAME[s.machine]}` };
    case "set_builtin":
      return { key: "builtin", label: BUILTIN_STEP[s.to] };
    case "update_input_geometry":
      return { key: "geometry", label: "Teclado y mouse" };
  }
}

export function groupSteps(desk: Desk, steps: Step[]): StepGroup[] {
  const groups: (StepGroup & { key: string })[] = [];
  steps.forEach((s, i) => {
    const d = describeStep(desk, s);
    const last = groups[groups.length - 1];
    if (last && last.key === d.key) last.steps.push(i);
    else groups.push({ key: d.key, label: d.label, steps: [i] });
  });
  return groups.map(({ label, steps }) => ({ label, steps }));
}

export type Outcome =
  | { kind: "running" }
  | { kind: "rolling_back"; error: string }
  | { kind: "committed" }
  | { kind: "rolled_back"; error: string }
  | { kind: "inconsistent"; error: string };

export interface Progress {
  txn: number;
  steps: Step[];
  status: StepStatus[];
  outcome: Outcome;
}

/** Avanza el progreso con un aviso. Los avisos de otra transacción se ignoran. */
export function advance(p: Progress | null, e: TxnView): Progress | null {
  if (e.type === "started") {
    return { txn: e.txn, steps: e.steps, status: e.steps.map(() => "pending"), outcome: { kind: "running" } };
  }
  if (!p || p.txn !== e.txn) return p;
  const status = [...p.status];
  const error = "error" in p.outcome ? p.outcome.error : "";
  switch (e.type) {
    case "step_started":
      status[e.index] = "run";
      return { ...p, status };
    case "step_done":
      status[e.index] = "done";
      return { ...p, status };
    case "step_failed":
      status[e.index] = "failed";
      return { ...p, status, outcome: { kind: "rolling_back", error: e.error } };
    case "rolling_back":
      return { ...p, outcome: { kind: "rolling_back", error } };
    case "committed":
      return { ...p, outcome: { kind: "committed" } };
    case "rolled_back":
      return { ...p, outcome: { kind: "rolled_back", error } };
    case "inconsistent": {
      const undo = e.undo_failures.map((f) => f.error).join("; ");
      return { ...p, outcome: { kind: "inconsistent", error: undo ? `${error}; al deshacer: ${undo}` : error } };
    }
  }
}

export function groupStatus(p: Progress, g: StepGroup): StepStatus {
  const s = g.steps.map((i) => p.status[i]);
  if (s.includes("failed")) return "failed";
  if (s.includes("run")) return "run";
  if (s.every((x) => x === "done")) return "done";
  // A medias: el grupo sigue en curso aunque su siguiente paso aún no arranque.
  if (s.includes("done")) return "run";
  return "pending";
}

export function isRunning(p: Progress | null): boolean {
  return p !== null && (p.outcome.kind === "running" || p.outcome.kind === "rolling_back");
}

/** Estado de la PC para el botón de encendido: arrancando mientras corre su encendido. */
export function pcPower(state: DeskState, p: Progress | null): "on" | "off" | "boot" {
  if (state.pc_on) return "on";
  if (p && isRunning(p) && p.steps.some((s, i) => s.step === "power_on_pc" && p.status[i] === "run")) return "boot";
  return "off";
}
