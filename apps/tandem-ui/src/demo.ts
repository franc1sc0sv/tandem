// Agente simulado para desarrollar y sacar capturas en el navegador (`pnpm dev`, `/?demo`).
// Solo se carga en desarrollo: el build de la app nunca lo incluye.
//
// `?demo&agent=off` simula que tandemd no corre; `?demo&pc=on` arranca con la PC encendida;
// `?demo&keys=busy` simula que el sistema no dejó registrar el atajo de Compartido;
// `?demo&setup` abre la guía de permisos del primer arranque (`&setup=fail`: tandemd no se
// pudo instalar); `?demo&peer=none`, un agente sin par emparejado. Emparejar desde ahí simula
// a la otra máquina: aparece el código y, al confirmar, confirma ella también
// (`&pair=reject`: la otra máquina rechaza; `&pair=code`: arranca mostrando el código).
// `?demo&hub=pc`, el hub en la PC; `?demo&perms=missing`, a la Mac le falta el permiso de
// Accesibilidad. `?demo&probe=question`, la pregunta de respaldo de una calibración sin la
// otra máquina (`&ask=switch`: el cambio a mano); la sonda a pantalla completa es otra página,
// `/probe.html?demo`. `?demo&addmon` abre «Agregar monitor»: el 34" ya configurado, el 32"
// sin perfil (calibrarlo simula el recorrido del motor, con la pregunta del cambio a mano) y
// un monitor que no está en la configuración (`&calib=fail`: ningún código de HDMI funciona).

import type { Bridge } from "./bridge";
import type {
  Action,
  ActionResult,
  Bindings,
  CalibrationStep,
  CalibrationView,
  Desk,
  DeskState,
  DisplayView,
  Machine,
  Mode,
  Notice,
  PairingView,
  PeerView,
  SetupView,
  ShortcutAction,
  ShortcutsView,
  Snapshot,
  Step,
} from "./model";
import { demoProbe } from "./probe";
import { builtinOf, modeRequest, type ModeKey } from "./scene";
import { ACTIONS, DEFAULT_BINDINGS, readCombo } from "./shortcuts";

const STEP_MS = 480;
const PAIR_FOUND_MS = 1400;
const PAIR_OTHER_MS = 1200;
const DEMO_CODE = "418027";

const DESK: Desk = {
  monitors: [
    { id: "m34", label: '34"', wiring: { pc: "displayport", mac: "hdmi" } },
    { id: "m32", label: '32"', wiring: { pc: "displayport", mac: "hdmi" } },
  ],
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const CALIB_MS = 650;
const PROFILE_32 = `# Perfil DDC de Tandem. Formato: docs/dev/ddc-profiles.md
pnp_id = "SAM7489"
name = "Odyssey G5 (SAM7489)"
edid_names = ["Odyssey G5"]
generic_name = true
notes = "Calibrado con Tandem: 2 códigos probados, vuelta a dp verificada."
verified_by = "calibración de Tandem"
verified_on = "2026-10-16"

[codes]
hdmi = 0x05
display_port = 0x0F

[reported]
hdmi = 0x05
display_port = 0x0F
`;

function demoDisplays(): DisplayView[] {
  return [
    { index: 0, name: "LC34G55T", pnp_id: "SAM711A", monitor: "m34", profile: { kind: "configured", hdmi: 0x06, display_port: 0x09 } },
    { index: 1, name: "Odyssey G5", pnp_id: "SAM7489", monitor: "m32", profile: { kind: "none" } },
    { index: 2, name: "U2720Q", pnp_id: "DEL41A3", monitor: null, profile: { kind: "none" } },
  ];
}

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
  const PAIRED: PeerView = { kind: "paired", name: "PC", link: "usb4" };
  let peer: PeerView = params.get("peer") === "none" ? { kind: "unpaired" } : PAIRED;
  let pairing: PairingView =
    params.get("pair") === "code" ? { kind: "comparing", code: DEMO_CODE, peer: "PC", confirmed: false } : { kind: "idle" };
  let pairTimer: number | undefined;
  const snapshot = (): Snapshot =>
    params.get("agent") === "off"
      ? { kind: "unavailable", endpoint: "~/Library/Application Support/Tandem/tandemd.sock", reason: "tandemd no está corriendo: no hay nadie escuchando en la IPC local" }
      : {
          kind: "connected",
          agent: "mac",
          desk: DESK,
          state,
          peer,
          pairing,
          input: {
            server: params.get("hub") === "pc" ? "pc" : "mac",
            hub_here: params.get("hub") !== "pc",
            session: state.pc_on && peer.kind === "paired",
            locked: false,
            missing: params.get("perms") === "missing" ? ["accessibility"] : [],
            problem: null,
          },
        };
  let keys: ShortcutsView = {
    bindings: { ...DEFAULT_BINDINGS },
    failed: params.get("keys") === "busy" ? { shared: "el sistema no dejó registrar Ctrl+Alt+Digit2 (¿lo usa otra app?)" } : {},
    warning: null,
    file: "~/Library/Application Support/Tandem/ui.toml",
  };
  const setupParam = params.get("setup");
  let setup: SetupView = {
    available: setupParam !== null,
    show: setupParam !== null,
    permissions: setupParam !== null ? ["accessibility", "input_monitoring", "local_network"] : [],
    agent:
      setupParam === "fail"
        ? { ok: false, text: "tandemd no se instaló: la app corre desde la imagen de disco; cópiala a Aplicaciones y ábrela desde ahí" }
        : { ok: true, text: "tandemd quedó instalado: arranca solo al iniciar sesión" },
  };
  const messages: ((text: string) => void)[] = [];
  const shortcutsCbs: ((view: ShortcutsView) => void)[] = [];
  const snapshots: ((s: Snapshot) => void)[] = [];
  const notices: ((n: Notice) => void)[] = [];
  const emit = (n: Notice) => notices.forEach((cb) => cb(n));
  let txn = 0;
  let busy = false;
  let displays = demoDisplays();
  let calibrating: { cancelled: boolean } | null = null;
  /** La respuesta a la pregunta del cambio a mano que espera la calibración simulada. */
  let answer: ((seen: boolean) => void) | null = null;

  /** El recorrido del motor (T-077) con el 32": la vuelta primero, después el HDMI. */
  async function calibrate(d: DisplayView, monitor: string | null) {
    const run = { cancelled: false };
    calibrating = run;
    busy = true;
    const say = (event: CalibrationView) => emit({ kind: "calibration", event });
    const step = async (s: CalibrationStep, ms = CALIB_MS) => {
      say({ type: "progress", step: s });
      await sleep(ms);
      return !run.cancelled;
    };
    const fail = params.get("calib") === "fail";
    const finish = (event: CalibrationView) => {
      busy = false;
      calibrating = null;
      say(event);
    };
    say({ type: "started", display: d.index, name: d.name, monitor, start: "displayport" });
    if (!(await step({ kind: "reading" }))) return finish({ type: "finished", outcome: { kind: "cancelled" } });
    say({ type: "progress", step: { kind: "user_switch", input: "hdmi" } });
    // Sin la otra máquina, esta ventana pregunta si el monitor cambió.
    const seen = await new Promise<boolean>((resolve) => {
      answer = resolve;
      emit({
        kind: "probe",
        event: { type: "show", id: 90, monitor: d.name, input: "hdmi", ask: { kind: "user_switch" }, timeout_ms: 120000, place: { kind: "question" } },
      });
    });
    answer = null;
    emit({ kind: "probe", event: { type: "hide", id: 90 } });
    if (run.cancelled) return finish({ type: "finished", outcome: { kind: "cancelled" } });
    if (!seen) return finish({ type: "finished", outcome: { kind: "failed", reason: "no se confirmó el cambio manual a hdmi desde el menú del monitor" } });
    const steps: CalibrationStep[] = [
      { kind: "testing", input: "displayport" },
      { kind: "trying", input: "displayport", code: 0x0f, attempt: 1, of: 31 },
      { kind: "seen", input: "displayport", code: 0x0f, seen: true },
      { kind: "found", input: "displayport", code: 0x0f },
      { kind: "testing", input: "hdmi" },
    ];
    for (const s of steps) if (!(await step(s))) return finish({ type: "finished", outcome: { kind: "cancelled" } });
    if (fail) {
      for (const [i, code] of [0x11, 0x12, 0x01].entries()) {
        if (!(await step({ kind: "trying", input: "hdmi", code, attempt: i + 1, of: 3 }))) return finish({ type: "finished", outcome: { kind: "cancelled" } });
        await step({ kind: "seen", input: "hdmi", code, seen: false }, 200);
        await step({ kind: "returned", code: 0x0f }, 300);
      }
      return finish({ type: "finished", outcome: { kind: "failed", reason: "ningún código funcionó para hdmi" } });
    }
    const last: CalibrationStep[] = [
      { kind: "trying", input: "hdmi", code: 0x05, attempt: 1, of: 30 },
      { kind: "seen", input: "hdmi", code: 0x05, seen: true },
      { kind: "returned", code: 0x0f },
      { kind: "found", input: "hdmi", code: 0x05 },
    ];
    for (const s of last) if (!(await step(s))) return finish({ type: "finished", outcome: { kind: "cancelled" } });
    displays = displays.map((x) => (x.index === d.index ? { ...x, monitor, profile: { kind: "configured", hdmi: 0x05, display_port: 0x0f } } : x));
    finish({
      type: "finished",
      outcome: {
        kind: "saved",
        name: "Odyssey G5 (SAM7489)",
        hdmi: 0x05,
        display_port: 0x0f,
        attempts: 2,
        monitor,
        file: "C:\\Users\\PC\\AppData\\Roaming\\Tandem\\profiles\\ddc\\SAM7489.toml",
        toml: PROFILE_32,
        notes: monitor ? [] : ["este monitor no está en tandem.toml: agrégalo con su `edid_model` para que los cambios de modo lo usen"],
      },
    });
  }

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

  function setPairing(next: PairingView) {
    pairing = next;
    snapshots.forEach((cb) => cb(snapshot()));
  }

  /** Hace de la otra máquina: la encuentra, y confirma (o rechaza) después de esta. */
  function pairAction(kind: "start_pairing" | "confirm_pairing" | "cancel_pairing" | "unpair"): ActionResult {
    switch (kind) {
      case "start_pairing":
        if (peer.kind === "paired") return { kind: "rejected", reason: `ya está emparejado con ${peer.name}: desemparéjalo primero` };
        if (pairing.kind === "searching" || pairing.kind === "comparing") return { kind: "rejected", reason: "ya hay un emparejamiento en curso" };
        setPairing({ kind: "searching" });
        pairTimer = window.setTimeout(() => setPairing({ kind: "comparing", code: DEMO_CODE, peer: "PC", confirmed: false }), PAIR_FOUND_MS);
        return { kind: "done" };
      case "confirm_pairing":
        if (pairing.kind !== "comparing" || pairing.confirmed) return { kind: "rejected", reason: "no hay ningún código que confirmar" };
        setPairing({ ...pairing, confirmed: true });
        pairTimer = window.setTimeout(() => {
          if (params.get("pair") === "reject") {
            setPairing({ kind: "failed", reason: "se rechazó en la PC: no se emparejó" });
            return;
          }
          peer = PAIRED;
          setPairing({ kind: "done", peer: "PC" });
        }, PAIR_OTHER_MS);
        return { kind: "done" };
      case "cancel_pairing":
        window.clearTimeout(pairTimer);
        setPairing({ kind: "idle" });
        return { kind: "done" };
      case "unpair":
        if (peer.kind !== "paired") return { kind: "rejected", reason: "no hay ningún par emparejado" };
        peer = { kind: "unpaired" };
        setPairing({ kind: "idle" });
        return { kind: "done" };
    }
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
    onShortcuts: async (cb) => {
      shortcutsCbs.push(cb);
    },
    setShortcuts: async (bindings: Bindings) => {
      const used = Object.values(bindings).filter((b): b is string => !!b);
      if (new Set(used).size !== used.length) throw "dos acciones usan el mismo atajo";
      keys = { ...keys, bindings: { ...bindings }, failed: {} };
      shortcutsCbs.forEach((cb) => cb(keys));
      return keys;
    },
    showMain: async () => {},
    hideMain: async () => {
      messages.forEach((cb) => cb("Aquí la ventana se escondería (atajo atendido por la ventana)"));
    },
    hideTray: async () => {},
    setup: async () => setup,
    openSettings: async (p) => {
      messages.forEach((cb) => cb(`Aquí se abriría Ajustes del Sistema (${p})`));
    },
    finishSetup: async () => {
      setup = { ...setup, show: false };
      return setup;
    },
    snapshot: async () => snapshot(),
    retry: async () => {},
    onSnapshot: async (cb) => {
      snapshots.push(cb);
    },
    onNotice: async (cb) => {
      notices.push(cb);
      if (params.get("probe") === "question") {
        const probe = demoProbe(`${query}&place=question&timeout=60000`);
        window.setTimeout(() => cb({ kind: "probe", event: probe }), 300);
      }
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
        case "answer_probe":
          if (answer) {
            answer(a.seen);
            return { kind: "done" };
          }
          emit({ kind: "probe", event: { type: "hide", id: a.id } });
          messages.forEach((cb) => cb(a.seen ? "Sigue la calibración con el próximo paso" : "Se prueba el código siguiente"));
          return { kind: "done" };
        case "start_pairing":
        case "confirm_pairing":
        case "cancel_pairing":
        case "unpair":
          return pairAction(a.kind);
        case "list_displays":
          await sleep(250);
          return { kind: "displays", displays };
        case "start_calibration": {
          if (calibrating) return { kind: "rejected", reason: "ya hay una calibración en curso" };
          if (busy) return { kind: "rejected", reason: "ya hay un cambio en curso" };
          const d = displays.find((x) => x.index === a.display);
          if (!d) return { kind: "rejected", reason: `ya no hay un monitor ${a.display} en el bus: vuelve a buscar` };
          void calibrate(d, a.monitor ?? d.monitor);
          return { kind: "done" };
        }
        case "cancel_calibration":
          if (!calibrating) return { kind: "rejected", reason: "no hay ninguna calibración en curso" };
          calibrating.cancelled = true;
          answer?.(false);
          return { kind: "done" };
        case "use_profile":
          return { kind: "rejected", reason: "en el modo demo ningún monitor tiene perfil en la base" };
      }
    },
  };
}
