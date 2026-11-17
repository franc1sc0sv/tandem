// Lo que la UI muestra del puesto, ya digerido para las pantallas. Lo llenan el agente real
// (`live.ts`, por la IPC de tandemd) o el modo demo (`demo.ts`). Las pantallas solo leen de
// aquí: así cada estado se puede dibujar y probar sin el agente.
//
// Los módulos y entornos llegan por la IPC con T-098/T-105; mientras tanto el agente real
// deja esas listas vacías y el modo demo las simula.

import type { Machine, Os } from "../model";

export type { Machine, Os };

export type ModuleState = "active" | "off" | "unavailable";

/** Un módulo de una máquina (D3): existe donde compila y dice si está activo y por qué no. */
export interface ModuleView {
  id: string;
  name: string;
  state: ModuleState;
  /** Qué hace en esta máquina, o por qué no está disponible. */
  detail: string;
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

export type TunnelState = "busy" | "idle" | "closed";

/** Un túnel de Claude (D8, solo lectura en la UI, D9). */
export interface TunnelView {
  id: string;
  project: string;
  title: string;
  from: Machine;
  to: Machine;
  state: TunnelState;
  processes: number;
}

export interface SceneView {
  id: string;
  name: string;
  /** Atajo global (`⌃⌥1`), o vacío. */
  key: string;
  /** Qué hace, en una línea. */
  desc: string;
  /** Qué máquina muestra (para la miniatura). */
  shows: Machine;
}

/** Los estados difíciles (`V3-Estados`). */
export type Trouble = "none" | "no-agent" | "peer-off" | "link-down" | "permissions";

export interface DeskView {
  /** `connecting` mientras no hay primer estado. */
  ready: boolean;
  trouble: Trouble;
  /** Por qué, en una línea (para el estado difícil). */
  troubleText: string;
  here: Machine;
  machines: Record<Machine, MachineView | null>;
  link: LinkView;
  tunnels: TunnelView[];
  scenes: SceneView[];
  activeScene: string | null;
}

/** Temas de eventos: cada pantalla se suscribe solo a lo que muestra. */
export type Topic = "machines" | "resources" | "link" | "tunnels" | "scenes" | "monitors";

export const ALL_TOPICS: Topic[] = ["machines", "resources", "link", "tunnels", "scenes", "monitors"];

/** Resultado de una acción pedida desde la UI. */
export type ActResult = { ok: true; text?: string } | { ok: false; reason: string };

/**
 * De dónde salen los datos. El store le dice qué temas quiere (los de la pantalla visible)
 * y recibe el estado nuevo cada vez que cambia.
 */
export interface Source {
  start(onDesk: (d: DeskView) => void): Promise<void>;
  /** Temas que la pantalla visible necesita en vivo; el resto se deja de mandar. */
  watch(topics: Topic[]): void;
  applyScene(id: string): Promise<ActResult>;
  setPower(machine: Machine, on: boolean): Promise<ActResult>;
  measureLink(): Promise<ActResult>;
  setModule(machine: Machine, module: string, on: boolean): Promise<ActResult>;
  /** Qué sabe hacer esta fuente hoy (el agente real aún no tiene escenas ni módulos). */
  can(what: "scenes" | "modules" | "power" | "measure"): boolean;
  stop(): void;
}
