// Espejo en TypeScript del JSON de `tandem_ui::session` (serde). Si cambia allá, cambia aquí;
// las pruebas de los dos lados fijan la forma.

export type Machine = "mac" | "pc";
export type Side = "left" | "right";

/** Una pantalla del acomodo declarado (DESIGN §7), en coordenadas nativas de su máquina. */
export interface ScreenSpec {
  machine: Machine;
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
}

/** El puesto compartido: el acomodo de pantallas para el teclado y el mouse. */
export interface Desk {
  screens: ScreenSpec[];
  /** De qué lado de la PC está la Mac; `offset` lo usa el editor del acomodo (T-096). */
  arrangement: { mac_side: Side; offset: number };
}

export interface DeskState {
  /** Qué máquinas están encendidas. La del agente siempre lo está para sí misma (T-084). */
  on: Record<Machine, boolean>;
}

/** Lo que tiene y sabe hacer una máquina, según su anuncio (`tandem_ui::session::MachineView`). */
export interface MachineCaps {
  machine: Machine;
  /** El nombre de la máquina en el sistema. */
  name: string;
  os: string;
  hub: boolean | null;
  input: {
    capture: boolean;
    inject: boolean;
    missing: ("accessibility" | "input_monitoring")[];
    /** El proceso del agente: macOS concede los permisos a ese binario exacto (T-090). */
    process: { exe: string; pid: number; started: number };
  };
  power: { wake_peer: boolean; wakeable: boolean; power_off: boolean };
  services: string[];
}

/** Esta máquina y la otra (`null` si nunca se la vio). */
export interface Caps {
  here: MachineCaps;
  peer: MachineCaps | null;
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
  /** Dónde está el teclado y el mouse (el campo se llama así en el agente). */
  server: Machine;
  /** Si el hub está en esta máquina; `null` sin detección. */
  hub_here: boolean | null;
  /** La sesión con el otro agente está establecida. */
  session: boolean;
  /** El cursor está bloqueado en su máquina (atajo de la que tiene el teclado). */
  locked: boolean;
  /** Permisos de macOS que le faltan a esta máquina. */
  missing: ("accessibility" | "input_monitoring")[];
  /** Por qué el motor de entrada de esta máquina no funciona, si no funciona. */
  problem: string | null;
  /** Se sabe dónde está el teclado (hub, configuración o asistente); si no, «sin definir». */
  defined: boolean;
}

export type Snapshot =
  | { kind: "connecting" }
  | { kind: "connected"; agent: Machine; desk: Desk; state: DeskState; peer: PeerView; pairing: PairingView; input: InputView; caps: Caps }
  | { kind: "unavailable"; endpoint: string | null; reason: string };

export type Step = { step: "power_on"; machine: Machine } | { step: "power_off"; machine: Machine };

export type TxnView =
  | { type: "started"; txn: number; steps: Step[] }
  | { type: "step_started"; txn: number; index: number }
  | { type: "step_done"; txn: number; index: number }
  | { type: "step_failed"; txn: number; index: number; error: string }
  | { type: "committed"; txn: number; state: DeskState }
  /** Falló un paso: `state` es lo que quedó hecho. */
  | { type: "failed"; txn: number; state: DeskState };

export type Notice =
  | { kind: "txn"; event: TxnView }
  | { kind: "key_probe"; event: { armed: boolean; heard: Machine | null } }
  | { kind: "power"; machine: Machine; reason: string };

export type Action =
  | { kind: "set_power"; machine: Machine; on: boolean }
  | { kind: "start_pairing" }
  | { kind: "confirm_pairing" }
  | { kind: "cancel_pairing" }
  | { kind: "unpair" }
  | { kind: "get_setup" }
  | { kind: "key_probe"; on: boolean }
  | { kind: "heard_key" }
  | { kind: "set_keyboard"; machine: Machine }
  | { kind: "ping_peer" }
  /** «Pedir permisos» (T-090): macOS muestra su diálogo a nombre de `tandemd`. */
  | { kind: "request_permissions" };

/** De dónde salió la configuración del agente (`tandem_proto::SetupInfo`). */
export interface AgentSetup {
  config_file: string | null;
  /** El agente la generó al arrancar por primera vez (T-082). */
  generated: boolean;
  port: number | null;
}

export type ActionResult =
  | { kind: "accepted"; txn: number }
  | { kind: "done" }
  | { kind: "rejected"; reason: string }
  | { kind: "unavailable"; reason: string }
  | { kind: "setup"; setup: AgentSetup }
  | { kind: "pong"; link: Link; millis: number };

// Atajos globales (`tandem_ui::shortcuts`).
export type ShortcutAction = "toggle_window";

/** Acción → combinación canónica (`Ctrl+Alt+Digit0`), o `null` si no tiene. */
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
