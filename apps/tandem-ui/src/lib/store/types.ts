// Lo que la UI muestra del puesto, ya digerido para las pantallas. Lo llenan el agente real
// (`live.ts`, por la IPC de tandemd) o el modo demo (`demo.ts`). Las pantallas solo leen de
// aquí: así cada estado se puede dibujar y probar sin el agente.
//
// Los módulos y entornos llegan por la IPC con T-098/T-105; mientras tanto el agente real
// deja esas listas vacías y el modo demo las simula.

import type { Arrangement, LayoutView } from "../acomodo";
import type { CrossSample } from "../crossing";
import type { Bindings, Machine, MonitorInput, Notice, Os, PairingView, PeerView, Permission, SceneJson, SetupView, ShortcutsView, TxnView, DragStatus } from "../model";
import type { PermissionState } from "../permissions";
import type { UpdateView } from "../updates";

export type { Machine, MonitorInput, Os };

export type ModuleState = "active" | "off" | "unavailable";

/** Un módulo de una máquina (D3): existe donde compila y dice si está activo y por qué no. */
export interface ModuleView {
  id: string;
  name: string;
  state: ModuleState;
  /** Qué hace en esta máquina, o por qué no está disponible. */
  detail: string;
  /** El usuario lo puede encender y apagar. */
  switchable: boolean;
}

/** Un entorno de ejecución detectado (T-105). */
export interface EnvView {
  id: string;
  kind: "shell" | "wsl" | "docker";
  name: string;
  /** El predeterminado de su clase. */
  default: boolean;
  ready: boolean;
  /** Herramientas, o por qué no está listo. */
  detail: string;
}

/** El equipo según el escaneo (D10). */
export interface HardwareView {
  cpu: string | null;
  /** Núcleos lógicos. */
  cores: number;
  /** RAM total, en bytes. */
  memory: number;
  /** `vram` en bytes; `null` si comparte la RAM. */
  gpu: { name: string; vram: number | null } | null;
  /** El equipo no tiene ventilador (el agente todavía no lo dice). */
  fanless?: boolean;
}

/** Recursos en vivo para la página Máquina (bytes y porcentajes 0–100). */
export interface LiveView {
  cpu: { percent: number; temp: number | null; history: number[] };
  ram: { used: number; total: number; history: number[] };
  gpu: {
    percent: number | null;
    vramUsed: number | null;
    vramTotal: number | null;
    temp: number | null;
    /** Ventiladores, 0–100, si el sistema lo dice. */
    fans: number | null;
    history: number[];
  } | null;
  disk: { free: number; total: number } | null;
  /** Batería, si tiene (el agente todavía no la manda). */
  battery: { percent: number; plugged: boolean } | null;
}

export interface Usage {
  /** 0–100. */
  percent: number;
  /** Texto corto a la derecha de la barra (`12.4/24 GB`, `18 %`). */
  label: string;
}

export interface Resources {
  cpu: Usage;
  ram: Usage;
  /** GPU o VRAM, si la máquina tiene. */
  gpu: Usage | null;
  gpuLabel: "GPU" | "VRAM";
  /** Últimas muestras de CPU (0–100), de la más vieja a la más nueva. */
  history: number[];
}

export type Power = "on" | "off" | "booting" | "lost";

export interface MachineView {
  id: Machine;
  name: string;
  os: Os;
  color: string;
  /** Línea bajo el nombre: sistema y lo más importante del equipo (sale del escaneo, D10). */
  detail: string;
  /** Es la máquina de esta ventana. */
  here: boolean;
  power: Power;
  resources: Resources | null;
  modules: ModuleView[];
  /** Sistema para mostrar («Windows 11», «macOS»). */
  system: string;
  hardware: HardwareView | null;
  environments: EnvView[];
  /** Recursos en vivo (página Máquina); `null` sin muestras. */
  live: LiveView | null;
  /** Teclados y mouse que anuncia, si los lista (el asistente, paso 4). */
  inputDevices?: { name: string; via?: string }[];
}

export type LinkKind = "usb4" | "ethernet" | "wifi";

export interface LinkView {
  /** Camino que lleva la entrada; `null` si no hay ninguno. */
  kind: LinkKind | null;
  /** Ancho de banda medido, en Gbps (`null` sin medir). */
  gbps: number | null;
  /** Latencia del enlace, en ms. */
  ms: number | null;
  /** p99 de la latencia de entrada, en ms. */
  inputP99: number | null;
}

/** Papel de un camino (T-102): lleva el tráfico, espera de respaldo, nunca respondió, o se cayó. */
export type PathState = "primary" | "backup" | "unplugged" | "down";

/** Un segundo del historial de un camino; `null` sin respuesta o sin entrada. */
export interface LatencyPoint {
  rttMs: number | null;
  inputP99Ms: number | null;
}

/** Un camino de la pantalla Red (`V3-Red`). */
export interface PathView {
  kind: LinkKind;
  state: PathState;
  rttMs: number | null;
  inputP50Ms: number | null;
  inputP99Ms: number | null;
  /** El p99 que se le tolera a la entrada por este camino (la línea de objetivo). */
  targetMs: number;
  /** Último ancho de banda medido, en Gbps. */
  gbps: number | null;
  /** Hasta 60 muestras, una por segundo, de la más vieja a la más nueva. */
  history: LatencyPoint[];
  /** Lo que se sabe del enlace («USB4 · 20 Gbps», «Wi-Fi 7 · casa»); `null` si el agente no lo dice. */
  detail: string | null;
}

/** Un cambio de camino («pasó a Wi-Fi en 0.6 s»). */
export interface PathSwitchView {
  /** Milisegundos Unix. */
  at: number;
  from: LinkKind | null;
  to: LinkKind | null;
  /** Cuánto tardó si el anterior se cayó; `null` si fue una subida sin corte. */
  tookMs: number | null;
}

/** La red con la otra máquina (lo que manda el agente con `WatchNetwork`). */
export interface NetworkView {
  /** USB-C, Ethernet y Wi-Fi, en ese orden. */
  paths: PathView[];
  /** Del más nuevo al más viejo (hasta 50). */
  switches: PathSwitchView[];
  /** El camino que se está midiendo. */
  measuring: LinkKind | null;
  /** Avisos con la orden exacta que el usuario ejecuta a mano (T-094). */
  notices: { interface: string; message: string; command: string }[];
  /** Cuándo es el último segundo del historial (milisegundos Unix): fecha los cambios de camino. */
  now: number;
}

/** p99 tolerado a la entrada por camino (DESIGN §7.1). */
export const INPUT_TARGET_MS: Record<LinkKind, number> = { usb4: 4, ethernet: 4, wifi: 20 };

const unplugged = (kind: LinkKind): PathView => ({
  kind,
  state: "unplugged",
  rttMs: null,
  inputP50Ms: null,
  inputP99Ms: null,
  targetMs: INPUT_TARGET_MS[kind],
  gbps: null,
  history: [],
  detail: null,
});

/** Sin red: los tres caminos sin cable. */
export const EMPTY_NETWORK: NetworkView = {
  paths: (["usb4", "ethernet", "wifi"] as LinkKind[]).map(unplugged),
  switches: [],
  measuring: null,
  notices: [],
  now: 0,
};

export type TunnelState = "busy" | "idle" | "closed";

/** Un túnel de Claude (D8, solo lectura en la UI, D9). */
export interface TunnelView {
  /** El id del agente (16 hex): viaja como texto porque no cabe en un número de JavaScript. */
  id: string;
  project: string;
  /** La carpeta del proyecto en el origen, si el agente la dio. */
  projectPath: string | null;
  title: string;
  from: Machine;
  to: Machine;
  state: TunnelState;
  /** Procesos corriendo ahora. */
  processes: number;
  /** La rama del workspace de git (T-107); `null` mientras el agente no la informe. */
  branch: string | null;
  /** Cuándo se abrió, en milisegundos Unix. */
  created: number;
  /** Desde cuándo está inactivo (ms Unix), si se sabe. */
  idleSince: number | null;
  /** Cuándo se cerró (ms Unix); solo en los cerrados. */
  closedAt: number | null;
  /** Dónde corren los procesos en la máquina destino («WSL · Ubuntu»); `null` si el agente no lo dice. */
  env: string | null;
  /** Comandos lanzados en el túnel; `null` si el agente no lo cuenta (la pantalla usa los que ve). */
  commands: number | null;
  /** Puertos reenviados; `null` si el agente no los lista. */
  ports: number | null;
  /** Bytes que fueron (`up`) y vinieron (`down`) por el túnel. */
  traffic: { up: number; down: number };
}

/** Un proceso de un túnel (`GetProcesses`). */
export interface ProcessRow {
  id: number;
  command: string;
  pid: number | null;
  /** Ms Unix. */
  startedAt: number;
  state: "running" | "exited" | "killed";
  code: number | null;
}

/** Hasta dónde se leyó la salida de cada flujo. */
export interface OutputCursor {
  stdout: number;
  stderr: number;
}

/** Lo que la salida de un proceso suma desde un cursor. */
export interface OutputChunk {
  text: string;
  next: OutputCursor;
  /** El proceso terminó y ya se entregó todo. */
  done: boolean;
}

export interface SceneView {
  id: string;
  name: string;
  /** Atajo global (`⌃⌥1`), o vacío. */
  key: string;
  /** Qué hace, en una línea. */
  desc: string;
  /** Qué máquina muestra (para la miniatura); `null` si la escena no cambia ningún monitor. */
  shows: Machine | null;
  /** Qué máquina muestra cada monitor compatible (los que la escena no toca no están). */
  monitors: { monitor: string; machine: Machine }[];
  /** Atajo global crudo (`KeyboardEvent.code`), para editarlo. */
  shortcut: string[];
  /** La escena tal como la guarda el puesto, para editarla sin perder lo que la pantalla no muestra. */
  raw: SceneJson | null;
}

/** Un evento del monitor, ya con su ícono y su texto (`V3-Monitores`). */
export interface MonitorEventView {
  /** Milisegundos Unix. */
  at: number;
  kind: "switch" | "button" | "signal" | "boot" | "power" | "failed" | "other";
  text: string;
}

/** Un monitor de la pantalla Monitores. D11: uno no compatible solo se muestra, con su razón. */
export interface MonitorUi {
  id: string;
  name: string;
  compat: { ok: true } | { ok: false; reason: string };
  /** Alguna máquina lo ve ahora. */
  connected: boolean;
  /** Quién lo lee y lo controla (D13). */
  owner: Machine | null;
  /** La entrada que está en pantalla (la real); `null` si no se sabe. */
  input: MonitorInput | null;
  /** A qué entrada va el cable de cada máquina, si se sabe. */
  ports: Partial<Record<Machine, MonitorInput>>;
  /** La máquina cuya señal se ve ahora, si se deduce de `input` y los cables. */
  shows: Machine | null;
  /** Del más nuevo al más viejo (hasta 3). */
  events: MonitorEventView[];
}

/** Emparejar con la otra máquina (lo que manda el agente). */
export interface PairState {
  peer: PeerView;
  pairing: PairingView;
}

/** Un permiso del sistema de esta máquina (macOS, T-090). */
export interface PermissionRow {
  id: Permission;
  state: PermissionState;
}

export interface PermissionsView {
  /** Esta máquina pide permisos (macOS); en Windows la lista queda vacía. */
  needed: boolean;
  rows: PermissionRow[];
  /** «Concede el permiso a este binario: …», si el agente lo dijo. */
  binary: string | null;
}

/** Los estados difíciles (`V3-Estados`). */
export type Trouble = "none" | "no-agent" | "peer-off" | "link-down" | "permissions";

/** Teclado y mouse compartidos (Ajustes → Teclado y mouse). */
export interface PrefsView {
  cmdCtrl: boolean;
  dragFiles: boolean;
  /** El portapapeles compartido está encendido en las dos máquinas. */
  clipboard: boolean;
}

/** Un cambio del puesto hecho desde la otra máquina (D5), para el aviso «Deshacer». */
export interface DeskChange {
  by: Machine;
  rev: number;
  summary: string;
  undoTo: number;
}

/** El avance de una copia de archivos entre las máquinas (aviso con progreso). */
export interface TransferView {
  upload: boolean;
  path: string;
  done: number;
  total: number;
  finished: boolean;
}

/** Dónde están el archivo de configuración y los logs de esta máquina. */
export interface FilesView {
  config: string;
  logs: string;
}

export interface DeskView {
  prefs: PrefsView;
  /** El atajo de bloqueo del puesto (`KeyboardEvent.code`); vacío = sin atajo. */
  lockShortcut: string[];
  /** `connecting` mientras no hay primer estado. */
  ready: boolean;
  /** El nombre del puesto (junto al logo en `V3-Maquina`); «Mi puesto» si no tiene. */
  deskName: string;
  trouble: Trouble;
  /** Por qué, en una línea (para el estado difícil). */
  troubleText: string;
  here: Machine;
  machines: Record<Machine, MachineView | null>;
  link: LinkView;
  /** Métricas por camino (pantalla Red); llegan solo con el tema `network`. */
  network: NetworkView;
  tunnels: TunnelView[];
  scenes: SceneView[];
  activeScene: string | null;
  /** Los monitores de las dos máquinas (D11: los no compatibles, marcados). */
  monitors: MonitorUi[];
  pair: PairState;
  permissions: PermissionsView;
  /** Las pantallas y el acomodo (D18). */
  layout: LayoutView;
}

/** Temas de eventos: cada pantalla se suscribe solo a lo que muestra. */
export type Topic = "machines" | "resources" | "link" | "network" | "tunnels" | "scenes" | "monitors";

export const ALL_TOPICS: Topic[] = ["machines", "resources", "link", "network", "tunnels", "scenes", "monitors"];

/** Lo que una fuente puede o no hacer todavía. */
export type Capability = "scenes" | "modules" | "power" | "measure" | "crossing" | "machine" | "monitors" | "forget" | "cursor";

/** Lo que llega con el cursor en vivo (T-128): su posición o un cruce de borde. */
export type CursorEvent = Extract<Notice, { kind: "cursor" | "crossed" }>;

export type PairActionKind = "start_pairing" | "confirm_pairing" | "cancel_pairing" | "unpair";

/** Resultado de una acción pedida desde la UI. */
export type ActResult = { ok: true; text?: string } | { ok: false; reason: string };

/**
 * De dónde salen los datos. El store le dice qué temas quiere (los de la pantalla visible)
 * y recibe el estado nuevo cada vez que cambia.
 */
export interface Source {
  start(onDesk: (d: DeskView) => void, onChange?: (c: DeskChange) => void, onTransfer?: (t: TransferView) => void): Promise<void>;
  /** Teclado y mouse compartidos: se manda el par entero. */
  setPrefs(p: { cmdCtrl: boolean; dragFiles: boolean }): Promise<ActResult>;
  /** El atajo de bloqueo del puesto (vacío = sin atajo). */
  setLockShortcut(lock: string[]): Promise<ActResult>;
  /** «Deshacer» el cambio de la otra máquina: vuelve el puesto a la versión `rev`. */
  undoDesk(rev: number): Promise<ActResult>;
  /** Archivo de configuración y logs (Ajustes → Avanzado). */
  files(): Promise<FilesView>;
  /** Muestra en el explorador el archivo de configuración o la carpeta de logs. */
  openFiles(target: "config" | "logs"): Promise<ActResult>;
  /** Temas que la pantalla visible necesita en vivo; el resto se deja de mandar. */
  watch(topics: Topic[]): void;
  applyScene(id: string): Promise<ActResult>;
  /** Cambiar la entrada de un monitor compatible: una transacción con el veredicto del firmware. */
  switchMonitor(monitor: string, input: MonitorInput): Promise<ActResult>;
  /** Crear (`isNew`) o guardar una escena en el puesto compartido: llega a la otra máquina. */
  saveScene(scene: SceneJson, isNew: boolean): Promise<ActResult>;
  /** Olvidar un monitor no compatible que ya no está. */
  forgetMonitor(id: string): Promise<ActResult>;
  /**
   * El avance de cada transacción (escena, monitor, energía) hasta que se llama a la función
   * que devuelve. `at` es la hora del evento y `origin` el título del `started`, si la fuente los
   * fija (el demo, para capturar).
   */
  watchTxn(onEvent: (e: TxnView, at?: number, origin?: { title: string; doneText: string }) => void): Promise<() => void>;
  /** Es el modo demo: las pantallas de referencia (Estados) muestran su galería completa. */
  readonly demo?: boolean;
  /** El avance de los arrastres de archivos entre las máquinas; las fuentes sin agente no lo tienen. */
  watchDrag?(onStatus: (s: DragStatus) => void): Promise<() => void>;
  /** La hora de la fuente si no es la del reloj (el demo quieto, para capturas deterministas). */
  now?(): number;
  setPower(machine: Machine, on: boolean): Promise<ActResult>;
  /** Medir el ancho de banda de un camino (por omisión, el que lleva el tráfico). */
  measureLink(path?: LinkKind): Promise<ActResult>;
  setModule(machine: Machine, module: string, on: boolean): Promise<ActResult>;
  /** Volver a escanear los entornos de `machine` (lo nuevo llega con el puesto). */
  rescan(machine: Machine): Promise<ActResult>;
  /** Nombre o color de una máquina (el puesto compartido, D4/D17). */
  editMachine(machine: Machine, edit: { name?: string; color?: string }): Promise<ActResult>;
  /** Emparejar (`start_pairing`, `confirm_pairing`, `cancel_pairing`) o desemparejar. */
  pairAction(kind: PairActionKind): Promise<ActResult>;
  /** macOS muestra su diálogo de permisos a nombre de tandemd. */
  requestPermissions(): Promise<ActResult>;
  /** Abre el panel de Ajustes del Sistema que concede el permiso. */
  openPermission(p: Permission): Promise<ActResult>;
  /** Reinicia tandemd para que valgan los permisos recién dados. */
  restartAgent(): Promise<ActResult>;
  /** La guía de permisos del primer arranque (macOS): si hay que abrirla sola. */
  setup(): Promise<SetupView>;
  /** Marca la guía como vista. */
  finishSetup(): Promise<void>;
  shortcuts(): Promise<ShortcutsView>;
  /** Rechaza con la razón si no se pueden usar. */
  setShortcuts(b: Bindings): Promise<ShortcutsView>;
  /** El atajo de la ventana atendido por ella misma (si el sistema no lo dejó registrar). */
  hideMain(): Promise<void>;
  /** Ajustes → Actualizaciones (T-122). `onChange` recibe cada cambio (búsqueda, avance). */
  updates(onChange: (v: UpdateView) => void): Promise<UpdateView>;
  checkUpdates(): Promise<UpdateView>;
  /** Rechaza con la razón si no se puede empezar. */
  updateAll(): Promise<UpdateView>;
  setAutoUpdate(on: boolean): Promise<UpdateView>;
  /** Guarda el acomodo en el puesto compartido (T-099): llega a la otra máquina. */
  setArrangement(a: Arrangement): Promise<ActResult>;
  /**
   * La prueba de cruce: `onCross` recibe cada cruce del cursor entre las dos máquinas hasta
   * que se llama a la función que devuelve. Rechaza si la fuente no informa cruces.
   */
  watchCrossing(onCross: (s: CrossSample) => void): Promise<() => void>;
  /**
   * El cursor en vivo (a 30 Hz como mucho) mientras no se llame a la función que devuelve.
   * Rechaza si el agente no lo ofrece.
   */
  watchCursor(onEvent: (e: CursorEvent) => void): Promise<() => void>;
  /**
   * Escucha una tecla o un movimiento en cada máquina (paso «Teclado y mouse»): `onHeard`
   * recibe la máquina que lo oyó, hasta que se llama a la función que devuelve.
   */
  watchKeyProbe(onHeard: (m: Machine) => void): Promise<() => void>;
  /** Tiempo para cruzar, si la fuente lo cambia (el demo, para capturar el tiempo agotado). */
  readonly crossTimeoutMs?: number;
  /** Los procesos de un túnel, también los que terminaron hace poco (T-104). */
  processes(tunnel: string): Promise<ProcessRow[]>;
  /** La salida de un proceso desde `from` (T-104). */
  output(tunnel: string, process: number, from: OutputCursor): Promise<OutputChunk>;
  /** Qué sabe hacer esta fuente hoy (el agente real aún no tiene escenas ni módulos). */
  can(what: Capability): boolean;
  stop(): void;
}
