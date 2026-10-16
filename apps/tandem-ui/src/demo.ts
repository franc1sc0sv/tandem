// Agente simulado para desarrollar y sacar capturas en el navegador (`pnpm dev`, `/?demo`).
// Solo se carga en desarrollo: el build de la app nunca lo incluye.
//
// `?demo&agent=off` simula que tandemd no corre; `?demo&pc=on` arranca con la PC encendida;
// `?demo&keys=busy` simula que el sistema no dejó registrar el atajo de Compartido.

import type { Bridge } from "./bridge";
import type {
  Action,
  ActionResult,
  Bindings,
  Desk,
  DeskState,
  Machine,
  Mode,
  Notice,
  ShortcutAction,
  ShortcutsView,
  Snapshot,
  Step,
} from "./model";
import { builtinOf, modeRequest, type ModeKey } from "./scene";
import { ACTIONS, DEFAULT_BINDINGS, readCombo } from "./shortcuts";

const STEP_MS = 480;

const DESK: Desk = {
  monitors: [
    { id: "m34", label: '34"', wiring: { pc: "displayport", mac: "hdmi" } },
    { id: "m32", label: '32"', wiring: { pc: "displayport", mac: "hdmi" } },
  ],
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Mismo orden de pasos que `Plan::to_mode` de tandem-core. */
function plan(state: DeskState, target: Record<string, Machine>, powerOff = false): Step[] {
  const steps: Step[] = [];
  if (!state.pc_on && Object.values(target).includes("pc")) steps.push({ step: "power_on_pc" });
  const next: DeskState = { ...state, owner: { ...state.owner } };
  for (const m of DESK.monitors) {
    const from = state.owner[m.id];
    const to = target[m.id];
    if (from === to) continue;
    steps.push({ step: "switch_input", monitor: m.id, from, to, input: m.wiring[to]! });
    steps.push({ step: "attach", machine: to, monitor: m.id });
    steps.push({ step: "detach", machine: from, monitor: m.id });
    next.owner[m.id] = to;
  }
  const before = builtinOf(DESK, state).effective;
  const after = builtinOf(DESK, next).effective;
  if (before !== after) steps.push({ step: "set_builtin", from: before, to: after });
  if (steps.length) steps.push({ step: "update_input_geometry" });
  if (powerOff && state.pc_on) steps.push({ step: "power_off_pc" });
  return steps;
}

function assignment(mode: Mode): Record<string, Machine> {
  return Object.fromEntries(
    DESK.monitors.map((m) => [
      m.id,
      mode.mode === "solo_mac" ? "mac" : mode.mode === "solo_pc" ? "pc" : (mode.assign[m.id] ?? "mac"),
    ]),
  );
}

export function demoBridge(query: string): Bridge {
  const params = new URLSearchParams(query);
  let state: DeskState = {
    owner: { m34: "mac", m32: "mac" },
    pc_on: params.get("pc") === "on",
    builtin_pref: "off",
  };
  const snapshot = (): Snapshot =>
    params.get("agent") === "off"
      ? { kind: "unavailable", endpoint: "~/Library/Application Support/Tandem/tandemd.sock", reason: "tandemd no está corriendo: no hay nadie escuchando en la IPC local" }
      : { kind: "connected", agent: "mac", desk: DESK, state };
  let keys: ShortcutsView = {
    bindings: { ...DEFAULT_BINDINGS },
    failed: params.get("keys") === "busy" ? { shared: "el sistema no dejó registrar Ctrl+Alt+Digit2 (¿lo usa otra app?)" } : {},
    warning: null,
    file: "~/Library/Application Support/Tandem/ui.toml",
  };
  const messages: ((text: string) => void)[] = [];
  const snapshots: ((s: Snapshot) => void)[] = [];
  const notices: ((n: Notice) => void)[] = [];
  const emit = (n: Notice) => notices.forEach((cb) => cb(n));
  let txn = 0;
  let busy = false;

  async function run(steps: Step[]) {
    const id = ++txn;
    busy = true;
    emit({ kind: "txn", event: { type: "started", txn: id, steps } });
    for (const [index, s] of steps.entries()) {
      emit({ kind: "txn", event: { type: "step_started", txn: id, index } });
      await sleep(s.step === "power_on_pc" ? STEP_MS * 4 : STEP_MS);
      state = structuredClone(state);
      if (s.step === "power_on_pc") state.pc_on = true;
      if (s.step === "power_off_pc") state.pc_on = false;
      if (s.step === "switch_input") state.owner[s.monitor] = s.to;
      emit({ kind: "txn", event: { type: "step_done", txn: id, index } });
    }
    busy = false;
    snapshots.forEach((cb) => cb(snapshot()));
    emit({ kind: "txn", event: { type: "committed", txn: id, state } });
  }

  function start(steps: Step[]): ActionResult {
    if (busy) return { kind: "rejected", reason: "ya hay un cambio en curso" };
    void run(steps);
    return { kind: "accepted", txn: txn + 1 };
  }

  // Hace de sistema: los atajos «globales» se atienden aquí, como lo haría el núcleo.
  const MODE_OF: Partial<Record<ShortcutAction, ModeKey>> = { solo_mac: "mac", shared: "shared", solo_pc: "pc" };
  addEventListener("keydown", (e) => {
    const c = readCombo(e);
    if (c.kind !== "ok") return;
    const action = ACTIONS.find((a) => keys.bindings[a] === c.accel && !(a in keys.failed));
    if (!action) return;
    e.preventDefault();
    const mode = MODE_OF[action];
    if (!mode) {
      messages.forEach((cb) => cb("Aquí la ventana se escondería (atajo global)"));
      return;
    }
    const r = start(plan(state, assignment(modeRequest(mode, DESK, state))));
    if (r.kind === "rejected") messages.forEach((cb) => cb(r.reason));
  });

  return {
    onMessage: async (cb) => {
      messages.push(cb);
    },
    shortcuts: async () => keys,
    setShortcuts: async (bindings: Bindings) => {
      const used = Object.values(bindings).filter((b): b is string => !!b);
      if (new Set(used).size !== used.length) throw "dos acciones usan el mismo atajo";
      keys = { ...keys, bindings: { ...bindings }, failed: {} };
      return keys;
    },
    showMain: async () => {},
    hideTray: async () => {},
    snapshot: async () => snapshot(),
    retry: async () => {},
    onSnapshot: async (cb) => {
      snapshots.push(cb);
    },
    onNotice: async (cb) => {
      notices.push(cb);
    },
    act: async (a: Action): Promise<ActionResult> => {
      switch (a.kind) {
        case "set_mode":
          return start(plan(state, assignment(a.mode)));
        case "move_monitor":
          return start(plan(state, { ...state.owner, [a.monitor]: a.to }));
        case "set_pc_power":
          if (a.on) return start(state.pc_on ? [] : [{ step: "power_on_pc" }]);
          return start(plan(state, assignment({ mode: "solo_mac" }), true));
        case "set_builtin":
          state = { ...state, builtin_pref: a.display };
          snapshots.forEach((cb) => cb(snapshot()));
          return { kind: "done" };
        case "answer_manual_switch":
          return { kind: "done" };
      }
    },
  };
}
