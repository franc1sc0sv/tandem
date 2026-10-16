// Lo que la ventana muestra a partir del estado de tandemd. Puro, para probarlo sin DOM.

export type Machine = "mac" | "pc";
export type BuiltinDisplay = "off" | "mirror" | "extend";

export interface DeskState {
  owner: Record<string, Machine>;
  pc_on: boolean;
  builtin_pref: BuiltinDisplay;
}

// Espejo de `tandem_ui::status::AgentStatus` (serde, `kind` como discriminante).
export type AgentStatus =
  | { kind: "connected"; agent: Machine; state: DeskState }
  | { kind: "unavailable"; endpoint: string | null; reason: string };

export interface Row {
  label: string;
  value: string;
  accent?: Machine;
}

export type View =
  | { kind: "connected"; pill: string; title: string; rows: Row[] }
  | { kind: "unavailable"; pill: string; title: string; reason: string; endpoint: string | null };

const MACHINE: Record<Machine, string> = { mac: "Mac", pc: "PC" };

const BUILTIN: Record<BuiltinDisplay, string> = {
  off: "Apagada",
  mirror: "Espejo",
  extend: "Extendida",
};

export function machineName(m: Machine): string {
  return MACHINE[m] ?? m;
}

export function describe(status: AgentStatus): View {
  if (status.kind === "unavailable") {
    return {
      kind: "unavailable",
      pill: "Sin agente",
      title: "No se pudo leer el estado de tandemd",
      reason: status.reason,
      endpoint: status.endpoint,
    };
  }
  const { agent, state } = status;
  const monitors = Object.keys(state.owner).sort();
  const rows: Row[] = [
    { label: "PC", value: state.pc_on ? "Encendida" : "Apagada", accent: "pc" },
    ...monitors.map((id) => {
      const m = state.owner[id];
      return { label: `Monitor ${id}`, value: machineName(m), accent: m };
    }),
    { label: "Pantalla integrada", value: BUILTIN[state.builtin_pref] ?? state.builtin_pref, accent: "mac" },
  ];
  return {
    kind: "connected",
    pill: `tandemd · ${machineName(agent)}`,
    title: monitors.length === 0 ? "Sin monitores configurados" : "Estado del puesto",
    rows,
  };
}

// Lo que se muestra si ni siquiera se pudo llamar al comando (por ejemplo, fuera de Tauri).
export function invokeFailed(err: unknown): AgentStatus {
  const detail = err instanceof Error ? err.message : String(err);
  return {
    kind: "unavailable",
    endpoint: null,
    reason: `la ventana no pudo preguntarle al núcleo de la app: ${detail}`,
  };
}
