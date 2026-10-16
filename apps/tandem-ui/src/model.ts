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

export type Snapshot =
  | { kind: "connecting" }
  | { kind: "connected"; agent: Machine; desk: Desk; state: DeskState }
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

export type Notice =
  | { kind: "txn"; event: TxnView }
  | { kind: "manual_switch"; event: ManualView }
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
  | { kind: "answer_manual_switch"; id: number; done: boolean };

export type ActionResult =
  | { kind: "accepted"; txn: number }
  | { kind: "done" }
  | { kind: "rejected"; reason: string }
  | { kind: "unavailable"; reason: string };
