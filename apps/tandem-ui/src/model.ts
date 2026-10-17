// Espejo en TypeScript del JSON de `tandem_ui::session` (serde). Si cambia allá, cambia aquí;
// las pruebas de los dos lados fijan la forma.

export type Machine = "mac" | "pc";
export type BuiltinDisplay = "off" | "mirror" | "extend";
export type Input = "hdmi" | "displayport";

export interface MonitorConfig {
  id: string;
  label: string;
  edid_model?: string | null;
  /** Los demás id PnP del mismo monitor: el EDID cambia según la entrada (T-087). */
  edid_aliases?: string[];
  wiring: Partial<Record<Machine, Input>>;
  /** Códigos DDC; sin ellos el monitor es de modo manual asistido. */
  ddc?: { hdmi: number; display_port: number } | null;
}

export interface Desk {
  monitors: MonitorConfig[];
}

/**
 * Por qué no se sabe lo que muestra un monitor (T-085): nunca se supo, es de un arranque
 * anterior, o el usuario dijo que está en negro.
 */
export type Doubt = "unknown" | "stale" | "blank";

export interface DeskState {
  owner: Record<string, Machine>;
  /** Qué máquinas están encendidas. La del agente siempre lo está para sí misma (T-084). */
  on: Record<Machine, boolean>;
  builtin_pref: BuiltinDisplay;
  /** Los monitores sin confirmar; el próximo cambio les manda la entrada (T-085). */
  doubt: Partial<Record<string, Doubt>>;
}

export interface CapsMonitor {
  monitor: string | null;
  pnp_id: string | null;
  name: string;
  switch: boolean;
  ddc: boolean;
  seen: boolean;
  model?: string | null;
  serial?: number | null;
  inches?: number | null;
  /** Por qué cable lo ve esa máquina, si el sistema lo dice. */
  output?: Input | null;
}

/** Lo que tiene y sabe hacer una máquina, según su anuncio (`tandem_ui::session::MachineView`). */
export interface MachineCaps {
  machine: Machine;
  /** El nombre de la máquina en el sistema. */
  name: string;
  os: string;
  /** Detectó sus monitores: los `seen` son los conectados ahora (T-087). */
  scanned: boolean;
  /** `seen`: la máquina lo detectó (conectado ahora; pista para preguntar lo que muestra, T-085). */
  monitors: CapsMonitor[];
  builtin: boolean;
  hub: boolean | null;
  input: { capture: boolean; inject: boolean; missing: ("accessibility" | "input_monitoring")[] };
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
  | { kind: "connected"; agent: Machine; desk: Desk; hidden: MonitorConfig[]; state: DeskState; peer: PeerView; pairing: PairingView; input: InputView; caps: Caps }
  | { kind: "unavailable"; endpoint: string | null; reason: string };

export type Step =
  | { step: "power_on"; machine: Machine }
  | { step: "power_off"; machine: Machine }
  | { step: "switch_input"; monitor: string; from: Machine; to: Machine; input: Input; by: Machine }
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
/** Qué se le pregunta al usuario; siempre contesta qué ve (T-081). */
export type ProbeAsk =
  | { kind: "candidate"; code: number }
  | { kind: "user_switch" }
  | { kind: "back"; code: number }
  | { kind: "verify"; code: number }
  | { kind: "stalled"; code: number };

/** La respuesta a una sonda: clic en la pantalla completa, o lo elegido en la pregunta. */
export type ProbeAnswer =
  | { kind: "seen" }
  | { kind: "machine"; machine: Machine }
  | { kind: "black" }
  | { kind: "stop" };

/** Dónde lo ve el agente, en coordenadas del sistema. */
export interface ProbeScreen {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * A pantalla completa en el monitor; en ninguna parte si el agente lo buscó y no está en el
 * escritorio (`offscreen`); o la pregunta «¿qué ves?» de la ventana principal.
 */
export type ProbePlace =
  | { kind: "monitor"; screen: ProbeScreen | null }
  | { kind: "offscreen" }
  | { kind: "question" };

export type ProbeShow = {
  type: "show";
  id: number;
  /** El nombre del monitor (EDID o descripción del sistema). */
  monitor: string;
  /** La entrada que debería verse. */
  input: Input;
  /** La máquina cableada a esa entrada. */
  machine: Machine;
  ask: ProbeAsk;
  timeout_ms: number;
  place: ProbePlace;
};

export type ProbeView = ProbeShow | { type: "hide"; id: number };

// «Agregar monitor» (T-080): `tandem_ui::session::{DisplayView, CalibrationView}`.

/** Los códigos que ya tiene un monitor, o que no tiene. */
export type DisplayProfile =
  | { kind: "none" }
  | { kind: "configured"; hdmi: number; display_port: number }
  | { kind: "known"; name: string; user: boolean; hdmi: number; display_port: number; toml: string };

/** Un monitor del bus DDC de la PC. `monitor` es el de la configuración al que corresponde. */
export interface DisplayView {
  index: number;
  name: string;
  pnp_id: string | null;
  monitor: string | null;
  profile: DisplayProfile;
}

export type CalibrationStep =
  | { kind: "reading" }
  | { kind: "user_switch"; input: Input }
  | { kind: "testing"; input: Input }
  | { kind: "trying"; input: Input; code: number; attempt: number; of: number }
  | { kind: "seen"; input: Input; code: number; seen: boolean }
  | { kind: "returned"; code: number }
  | { kind: "found"; input: Input; code: number }
  | { kind: "verifying" }
  | { kind: "verified"; input: Input; code: number };

export type CalibrationOutcome =
  | {
      kind: "saved";
      name: string;
      hdmi: number;
      display_port: number;
      attempts: number;
      monitor: string | null;
      /** Dónde quedó el perfil del usuario. */
      file: string | null;
      /** El perfil para exportarlo. */
      toml: string | null;
      notes: string[];
    }
  | { kind: "failed"; reason: string }
  | { kind: "cancelled" };

export type CalibrationView =
  | { type: "started"; display: number; name: string; monitor: string | null; start: Input }
  | { type: "progress"; step: CalibrationStep }
  | { type: "finished"; outcome: CalibrationOutcome };

export type Notice =
  | { kind: "txn"; event: TxnView }
  | { kind: "manual_switch"; event: ManualView }
  | { kind: "probe"; event: ProbeView }
  | { kind: "calibration"; event: CalibrationView }
  | { kind: "key_probe"; event: { armed: boolean; heard: Machine | null } }
  | { kind: "power"; machine: Machine; reason: string; stranded: string[] };

export type Mode =
  | { mode: "solo_mac" }
  | { mode: "solo_pc" }
  | { mode: "shared"; assign: Record<string, Machine> };

export type Action =
  | { kind: "set_mode"; mode: Mode }
  | { kind: "move_monitor"; monitor: string; to: Machine }
  | { kind: "set_builtin"; display: BuiltinDisplay }
  | { kind: "set_power"; machine: Machine; on: boolean }
  | { kind: "answer_manual_switch"; id: number; done: boolean }
  | { kind: "answer_probe"; id: number; answer: ProbeAnswer }
  | { kind: "start_pairing" }
  | { kind: "confirm_pairing" }
  | { kind: "cancel_pairing" }
  | { kind: "unpair" }
  | { kind: "list_displays" }
  | { kind: "start_calibration"; display: number; monitor: string | null }
  | { kind: "cancel_calibration" }
  | { kind: "use_profile"; display: number; monitor: string | null }
  /** `confirmed`: el usuario lo confirmó; sin esto el agente lo rechaza (T-087). */
  | { kind: "set_wiring"; monitor: string; wiring: Partial<Record<Machine, Input>>; confirmed?: boolean }
  | { kind: "get_setup" }
  /** Asistente (T-087). */
  | { kind: "scan_monitors" }
  | { kind: "add_monitors"; monitors: NewMonitor[] }
  | { kind: "remove_monitor"; monitor: string }
  | { kind: "reset_desk" }
  | { kind: "key_probe"; on: boolean }
  | { kind: "heard_key" }
  | { kind: "set_keyboard"; machine: Machine }
  | { kind: "ping_peer" }
  | { kind: "mark_manual"; monitor: string }
  /** «Aplicar modo» (T-085): manda la entrada de todos los monitores. */
  | { kind: "apply_mode"; mode: Mode }
  /** Lo que el usuario dice que muestra cada monitor; `null`: nada (en negro). */
  | { kind: "set_shown"; shows: Record<string, Machine | null> };

/** Un monitor físico que el asistente agrega al puesto (`tandem_proto::NewMonitor`). */
export interface NewMonitor {
  label: string;
  /** El id PnP de cada máquina, el de la PC primero. */
  pnp_ids: string[];
  wiring: Partial<Record<Machine, Input>>;
}

/** Con qué se vinculó la identidad de una máquina con la de la otra (`LinkBasis`). */
export type LinkBasis = "Serial" | "Model" | "Single";

export interface MonitorCandidate {
  label: string;
  pnp_ids: string[];
  here: CapsMonitor | null;
  peer: CapsMonitor | null;
  basis: LinkBasis;
  /** El cableado deducido de por dónde lo ve cada máquina; `null`, hay que preguntar. */
  wiring: Partial<Record<Machine, Input | null>>;
  in_desk: string | null;
  profile: string | null;
}

export interface MonitorScan {
  candidates: MonitorCandidate[];
  peer_known: boolean;
  /** Monitores del puesto que ninguna de las dos máquinas ve ahora. */
  disconnected: number;
}

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
  | { kind: "displays"; displays: DisplayView[] }
  | { kind: "setup"; setup: AgentSetup }
  | { kind: "scan"; scan: MonitorScan }
  | { kind: "pong"; link: Link; millis: number };

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
