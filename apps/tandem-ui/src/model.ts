// Espejo en TypeScript del JSON de `tandem_ui::session` (serde). Si cambia allá, cambia aquí;
// las pruebas de los dos lados fijan la forma.

export type Machine = "mac" | "pc";
export type BuiltinDisplay = "off" | "mirror" | "extend";
export type Input = "hdmi" | "displayport";

export interface MonitorConfig {
  id: string;
  label: string;
  edid_model?: string | null;
  wiring: Partial<Record<Machine, Input>>;
}

export interface Desk {
  monitors: MonitorConfig[];
}

export interface DeskState {
  owner: Record<string, Machine>;
  pc_on: boolean;
  builtin_pref: BuiltinDisplay;
}

export type Link = "usb4" | "ethernet" | "wifi";

/** El otro agente: sin par, esta máquina trabaja sola. `link` es `null` si no se lo ve. */
export type PeerView = { kind: "unpaired" } | { kind: "paired"; name: string; link: Link | null };

/** En qué va el emparejamiento (DESIGN §9): `code` son los 6 dígitos, con ceros a la izquierda. */
export type PairingView =
  | { kind: "idle" }
  | { kind: "searching" }
  | { kind: "comparing"; code: string; peer: string; confirmed: boolean }
  | { kind: "done"; peer: string }
  | { kind: "failed"; reason: string };

/** El teclado y mouse compartidos (`tandem_ui::session::InputView`). */
export interface InputView {
  /** La máquina que tiene el teclado y el mouse. */
  server: Machine;
  /** Si el hub está en esta máquina; `null` sin detección. */
  hub_here: boolean | null;
  /** La sesión con el otro agente está establecida. */
  session: boolean;
  /** El cursor está bloqueado en su máquina (atajo del servidor). */
  locked: boolean;
  /** Permisos de macOS que le faltan a esta máquina. */
  missing: ("accessibility" | "input_monitoring")[];
  /** Por qué el motor de entrada de esta máquina no funciona, si no funciona. */
  problem: string | null;
}

export type Snapshot =
  | { kind: "connecting" }
  | { kind: "connected"; agent: Machine; desk: Desk; state: DeskState; peer: PeerView; pairing: PairingView; input: InputView }
  | { kind: "unavailable"; endpoint: string | null; reason: string };

export type Step =
  | { step: "power_on_pc" }
  | { step: "power_off_pc" }
  | { step: "switch_input"; monitor: string; from: Machine; to: Machine; input: Input }
  | { step: "attach"; machine: Machine; monitor: string }
  | { step: "detach"; machine: Machine; monitor: string }
  | { step: "set_builtin"; from: BuiltinDisplay; to: BuiltinDisplay }
  | { step: "update_input_geometry" };

export type TxnView =
  | { type: "started"; txn: number; steps: Step[] }
  | { type: "step_started"; txn: number; index: number }
  | { type: "step_done"; txn: number; index: number }
  | { type: "step_failed"; txn: number; index: number; error: string }
  | { type: "rolling_back"; txn: number }
  | { type: "committed"; txn: number; state: DeskState }
  | { type: "rolled_back"; txn: number; state: DeskState }
  | { type: "inconsistent"; txn: number; undo_failures: { step: Step; error: string }[] };

export type ManualView =
  | { type: "requested"; id: number; monitor: string; label: string; to: Machine; input: Input }
  | { type: "closed"; id: number };

/** Qué confirma el usuario en una sonda de calibración (`tandem_ui::session::AskView`). */
export type ProbeAsk = { kind: "candidate"; code: number } | { kind: "user_switch" };

/** Dónde lo ve el agente, en coordenadas del sistema. */
export interface ProbeScreen {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A pantalla completa en el monitor, o la pregunta Sí/No de respaldo en la ventana principal. */
export type ProbePlace = { kind: "monitor"; screen: ProbeScreen | null } | { kind: "question" };

export type ProbeShow = {
  type: "show";
  id: number;
  /** El nombre del monitor (EDID o descripción del sistema). */
  monitor: string;
  /** La entrada que debería verse. */
  input: Input;
  ask: ProbeAsk;
  timeout_ms: number;
  place: ProbePlace;
};

export type ProbeView = ProbeShow | { type: "hide"; id: number };

export type Notice =
  | { kind: "txn"; event: TxnView }
  | { kind: "manual_switch"; event: ManualView }
  | { kind: "probe"; event: ProbeView }
  | { kind: "power"; reason: string; stranded: string[] };

export type Mode =
  | { mode: "solo_mac" }
  | { mode: "solo_pc" }
  | { mode: "shared"; assign: Record<string, Machine> };

export type Action =
  | { kind: "set_mode"; mode: Mode }
  | { kind: "move_monitor"; monitor: string; to: Machine }
  | { kind: "set_builtin"; display: BuiltinDisplay }
  | { kind: "set_pc_power"; on: boolean }
  | { kind: "answer_manual_switch"; id: number; done: boolean }
  | { kind: "answer_probe"; id: number; seen: boolean }
  | { kind: "start_pairing" }
  | { kind: "confirm_pairing" }
  | { kind: "cancel_pairing" }
  | { kind: "unpair" };

export type ActionResult =
  | { kind: "accepted"; txn: number }
  | { kind: "done" }
  | { kind: "rejected"; reason: string }
  | { kind: "unavailable"; reason: string };

// Atajos globales (`tandem_ui::shortcuts`).
export type ShortcutAction = "solo_mac" | "shared" | "solo_pc" | "toggle_window";

/** Acción → combinación canónica (`Ctrl+Alt+Digit1`), o `null` si no tiene. */
export type Bindings = Record<ShortcutAction, string | null>;

export interface ShortcutsView {
  bindings: Bindings;
  /** Los que el sistema no dejó registrar, con la razón: los atiende la ventana. */
  failed: Partial<Record<ShortcutAction, string>>;
  warning: string | null;
  file: string | null;
}

// Primer arranque en macOS (`tandem_ui::setup`).
export type Permission = "accessibility" | "input_monitoring" | "local_network";

export interface SetupView {
  /** Hay guía en este sistema (solo macOS). */
  available: boolean;
  /** Mostrarla al abrir: todavía no se terminó. */
  show: boolean;
  permissions: Permission[];
  /** Cómo quedó el agente de inicio de sesión (`tandemd`), si la app intentó instalarlo. */
  agent: { ok: boolean; text: string } | null;
}
