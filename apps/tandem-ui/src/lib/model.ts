// Espejo en TypeScript del JSON de `tandem_ui::session` (serde). Si cambia allá, cambia aquí;
// las pruebas de los dos lados fijan la forma.

/**
 * Los dos lados del puesto (T-097, DECISIONS A1): `a` es el de clave pública menor. Ningún
 * lado supone un sistema: nombre, sistema y color salen de su anuncio (`MachineInfo`).
 */
export type Machine = "a" | "b";
export type Os = "macos" | "windows";

/** Lo que una máquina dice de sí misma (`tandem_core::MachineInfo`). */
export interface MachineInfo {
  os: Os;
  name: string;
  /** Color de acento en CSS (`#rrggbb`), elegido al emparejar (D17). */
  color: string;
}
export type Side = "left" | "right" | "up" | "down";
export type Align = "start" | "center" | "end";

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
  /** Lo que se sabe de cada lado (`null` si todavía no se anunció). */
  machines: Record<Machine, MachineInfo | null>;
  screens: ScreenSpec[];
  /** De qué lado de `b` está `a` y con qué alineación (D18); `null` sin declarar: `a` a la
   * izquierda de `b` y todo el borde cruza (T-099). */
  arrangement: { a_side: Side; align: Align } | null;
  /** Nombre del puesto, si el agente lo manda (todavía no viaja: la UI usa «Mi puesto»). */
  name?: string | null;
}

export interface DeskState {
  /** Qué máquinas están encendidas. La del agente siempre lo está para sí misma (T-084). */
  on: Record<Machine, boolean>;
  /** La escena activa (D6), si sigue valiendo. */
  scene?: string | null;
}

/** Lo que tiene y sabe hacer una máquina, según su anuncio (`tandem_ui::session::MachineView`). */
export interface MachineCaps {
  machine: Machine;
  name: string;
  os: Os;
  color: string;
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
  /** Módulos de su sistema (T-098); opcionales para leer un agente de antes. */
  modules?: ModuleCaps[];
  /** Entornos de ejecución detectados (T-105). */
  environments?: EnvCaps[];
  /** El equipo según el escaneo (T-105). */
  hardware?: HardwareCaps;
}

/** Un módulo tal como lo anuncia su máquina (`tandem_ui::session::ModuleView`). */
export interface ModuleCaps {
  /** El id del puesto: `input`, `clipboard`, `secure_desktop`… */
  id: string;
  name: string;
  detail: string;
  state: "active" | "off" | "unavailable";
  reason: string | null;
  switchable: boolean;
}

/** Un entorno de ejecución (`tandem_ui::session::EnvView`). */
export interface EnvCaps {
  id: string;
  kind: "shell" | "wsl" | "docker";
  name: string;
  default: boolean;
  ready: boolean;
  detail: string;
  gpu: boolean;
}

export interface HardwareCaps {
  cpu: string | null;
  cores: number;
  /** RAM total, en bytes. */
  memory: number;
  gpus: { name: string; vram: number | null }[];
}

/** Una muestra de recursos (`tandem_proto::Resources`): porcentajes en décimas, bytes. */
export interface ResourceSample {
  cpu: { load: number; cores: number[]; temperature: number | null };
  memory: { used: number; total: number };
  gpus: { name: string; load: number | null; vram_used: number | null; vram_total: number | null; temperature: number | null }[];
  disk: { free: number; total: number } | null;
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

export type Step =
  | { step: "power_on"; machine: Machine }
  | { step: "power_off"; machine: Machine }
  | { step: "switch_monitor"; monitor: string; input: "display_port" | "hdmi" }
  /** Pasos de escena (T-100): qué máquina muestra un monitor, el acomodo y la geometría. */
  | { step: "show_machine"; monitor: string; machine: Machine }
  | { step: "arrange"; layout: unknown }
  | { step: "geometry" };

export type TxnView =
  | { type: "started"; txn: number; steps: Step[] }
  | { type: "step_started"; txn: number; index: number }
  | { type: "step_done"; txn: number; index: number }
  | { type: "step_failed"; txn: number; index: number; error: string }
  /** Vuelta atrás: un paso ya hecho se deshizo porque falló uno posterior. */
  | { type: "step_undone"; txn: number; index: number }
  | { type: "committed"; txn: number; state: DeskState }
  /** Falló un paso: `state` es lo que quedó hecho. */
  | { type: "failed"; txn: number; state: DeskState };

/** Borde del escritorio de una máquina (`tandem_core::Side`). */
export type CrossEdge = "left" | "right" | "up" | "down";

/** Dónde está el cursor, en las coordenadas nativas de su máquina (`tandem_proto::CursorSample`). */
export interface CursorSample {
  machine: Machine;
  /** Id de la pantalla que lo contiene; vacío si no se conocen. */
  screen: string;
  x: number;
  y: number;
}

export type Notice =
  | { kind: "txn"; event: TxnView }
  /** El cursor en vivo (T-128), a 30 Hz como mucho; solo llega con `watch_cursor`. */
  | { kind: "cursor"; sample: CursorSample }
  /** El cursor cruzó el borde `edge` de `from` hacia `to` (T-128). */
  | { kind: "crossed"; from: Machine; to: Machine; edge: CrossEdge }
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
  /** Recibir (o dejar de recibir) el cursor en vivo como avisos `cursor` y `crossed` (T-128). */
  | { kind: "watch_cursor"; on: boolean }
  /** «Pedir permisos» (T-090): macOS muestra su diálogo a nombre de `tandemd`. */
  | { kind: "request_permissions" }
  /** Guardar el acomodo en el puesto compartido (`EditDesk { Arrangement }`, T-099). */
  | { kind: "set_arrangement"; a_side: Side; align: Align }
  | { kind: "set_module"; machine: Machine; module: string; on: boolean }
  | { kind: "rescan"; machine: Machine }
  /** Recibir los recursos en vivo mientras la pantalla visible los muestra. */
  | { kind: "watch_resources"; on: boolean }
  /** Nombre o color de una máquina (el puesto compartido). */
  | { kind: "set_machine"; machine: Machine; name?: string; color?: string };

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
