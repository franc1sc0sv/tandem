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
// `?demo&desk` abre «Puesto»: la configuración que generó el agente; «Invertir cableado»
// cambia la del monitor en la escena. `?demo&as=pc` es la ventana de la PC (T-084): la PC
// a la izquierda, siempre encendida, con los monitores, y la Mac dormida a la derecha
// (`&mac=on`: despierta). `?demo&fresh` arranca sin estado guardado (T-085): nadie sabe lo
// que muestra cada monitor (`&fresh=stale`: lo último que se supo, de un arranque
// anterior); con la PC apagada se ofrece responder, con `&pc=on` también «Aplicar modo».
// `?demo&clean` es una instalación limpia (T-087): el puesto está vacío y el asistente se abre
// solo; la PC está enlazada y encendida (`&peer=none`: sin emparejar; `&basis=model`: los
// monitores no comparten número de serie, así que el vínculo se pregunta; `&kb=pc`: la tecla
// de la prueba llega a la PC; `&unplug=sam7489`: ese monitor no está conectado; `&extra`: un
// tercer monitor que solo ve la PC). El asistente se abre desde «Asistente».

import type { Bridge } from "./bridge";
import type {
  Action,
  CapsMonitor,
  ActionResult,
  Bindings,
  Caps,
  MachineCaps,
  CalibrationStep,
  CalibrationView,
  Desk,
  DeskState,
  DisplayView,
  MonitorCandidate,
  MonitorConfig,
  MonitorScan,
  Machine,
  Mode,
  Notice,
  PairingView,
  PeerView,
  ProbeAnswer,
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
    { id: "m34", label: '34"', edid_model: "SAM711A", wiring: { pc: "displayport", mac: "hdmi" } },
    { id: "m32", label: '32"', edid_model: "SAM7489", wiring: { pc: "displayport", mac: "hdmi" } },
  ],
};

/** El puesto real, como lo ve cada máquina: otro id PnP por entrada, el mismo número de serie. */
interface Physical {
  label: string;
  model: string;
  inches: number;
  serial: number;
  /** El id PnP que ve la PC (DisplayPort) y el que ve la Mac (HDMI). */
  pc: string;
  mac: string | null;
  profile: { name: string; hdmi: number; display_port: number } | null;
}

const PHYSICAL: Physical[] = [
  { label: '34"', model: "LC34G55T", inches: 34, serial: 0x4d304e30, pc: "SAM711A", mac: "SAM7119", profile: { name: "LC34G55T", hdmi: 0x06, display_port: 0x09 } },
  { label: '32"', model: "Odyssey G5", inches: 32, serial: 0x4d304e31, pc: "SAM7489", mac: "SAM748A", profile: { name: 'Odyssey G5 32" (SAM7489)', hdmi: 0x05, display_port: 0x0f } },
  // Solo lo ve la PC (cableado a ella nada más).
  { label: '27"', model: "U2720Q", inches: 27, serial: 0x41a3, pc: "DEL41A3", mac: null, profile: null },
];

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

/** Lo que anuncia cada máquina del puesto real: la PC cambia las entradas, la Mac tiene pantalla. */
function demoCaps(machine: Machine, missing: MachineCaps["input"]["missing"], monitors: CapsMonitor[]): MachineCaps {
  const pc = machine === "pc";
  return {
    machine,
    name: pc ? "ESCRITORIO" : "MacBook-Pro",
    os: pc ? "windows" : "macos",
    scanned: true,
    monitors,
    builtin: !pc,
    hub: null,
    input: { capture: missing.length === 0, inject: missing.length === 0, missing },
    power: { wake_peer: true, wakeable: true, power_off: pc },
    services: [],
  };
}

/** Quién cambia las entradas en el demo: la PC, como en el puesto real. */
const SWITCHER: Machine = "pc";

/**
 * Mismo orden de pasos que `Plan::to_mode` (y `Plan::power_off`) de tandem-core. A un
 * monitor sin confirmar (o con `all`, «Aplicar modo») se le manda la entrada igual (T-085).
 */
function plan(desk: Desk, state: DeskState, target: Record<string, Machine>, powerOff: Machine | null = null, all = false): Step[] {
  const steps: Step[] = [];
  const moves = desk.monitors.filter((m) => all || state.doubt[m.id] !== undefined || state.owner[m.id] !== target[m.id]);
  const needed = new Set<Machine>(Object.values(target));
  if (moves.length) needed.add(SWITCHER);
  const on = { ...state.on };
  if (moves.length || Object.values(target).some((m) => !state.on[m])) {
    for (const machine of ["mac", "pc"] as const) {
      if (needed.has(machine) && !on[machine]) {
        steps.push({ step: "power_on", machine });
        on[machine] = true;
      }
    }
  }
  const next: DeskState = { ...state, owner: { ...state.owner }, on };
  for (const m of moves) {
    const from = state.owner[m.id];
    const to = target[m.id];
    steps.push({ step: "switch_input", monitor: m.id, from, to, input: m.wiring[to]!, by: SWITCHER });
    const away: Machine = to === "mac" ? "pc" : "mac";
    if (on[to]) steps.push({ step: "attach", machine: to, monitor: m.id });
    if (on[away]) steps.push({ step: "detach", machine: away, monitor: m.id });
    next.owner[m.id] = to;
  }
  const before = builtinOf(desk, state).effective;
  const after = builtinOf(desk, next).effective;
  if (before !== after && on.mac) steps.push({ step: "set_builtin", from: before, to: after });
  if (steps.length) steps.push({ step: "update_input_geometry" });
  if (powerOff && (state.on[powerOff] || steps.some((s) => s.step === "power_on" && s.machine === powerOff))) {
    steps.push({ step: "power_off", machine: powerOff });
  }
  return steps;
}

function assignment(desk: Desk, mode: Mode): Record<string, Machine> {
  return Object.fromEntries(
    desk.monitors.map((m) => [
      m.id,
      mode.mode === "solo_mac" ? "mac" : mode.mode === "solo_pc" ? "pc" : (mode.assign[m.id] ?? "mac"),
    ]),
  );
}

export function demoBridge(query: string): Bridge {
  const params = new URLSearchParams(query);
  // La ventana de qué máquina es: la Mac por omisión, la PC con `&as=pc`.
  const agent: Machine = params.get("as") === "pc" ? "pc" : "mac";
  const peerMachine: Machine = agent === "mac" ? "pc" : "mac";
  // Instalación limpia (T-087): puesto vacío, la PC enlazada y encendida.
  const clean = params.has("clean");
  const peerOn = clean ? params.get(peerMachine) !== "off" : params.get(peerMachine) === "on";
  // El puesto es de este agente y cambia con el asistente.
  let desk: Desk = clean ? { monitors: [] } : structuredClone(DESK);
  // Sin estado guardado, lo inventado es «todo en la Mac» (`State::initial`).
  const fresh = clean ? "new" : params.get("fresh");
  const guess: Machine = fresh === null ? agent : fresh === "stale" ? "pc" : "mac";
  const doubtKind = fresh === "stale" ? "stale" : "unknown";
  let state: DeskState = {
    owner: Object.fromEntries(desk.monitors.map((m) => [m.id, guess])),
    on: { [agent]: true, [peerMachine]: peerOn } as Record<Machine, boolean>,
    builtin_pref: "off",
    doubt: (fresh === null ? {} : Object.fromEntries(desk.monitors.map((m) => [m.id, doubtKind]))) as DeskState["doubt"],
  };
  const missing: MachineCaps["input"]["missing"] = params.get("perms") === "missing" ? ["accessibility"] : [];
  // Lo conectado ahora: los dos de siempre, y con `&extra` uno que solo ve la PC.
  const unplugged = new Set((params.get("unplug") ?? "").split(",").filter(Boolean));
  const byModel = params.get("basis") === "model";
  const physical = PHYSICAL.filter((p) => p.mac !== null || params.has("extra"));
  const side = (p: Physical, machine: Machine): CapsMonitor | null => {
    const pnp = machine === "pc" ? p.pc : p.mac;
    if (!pnp) return null;
    const id = desk.monitors.find((m) => [m.edid_model, ...(m.edid_aliases ?? [])].includes(pnp))?.id ?? null;
    return {
      monitor: id,
      pnp_id: pnp,
      name: `${p.inches}"`,
      switch: machine === "pc",
      ddc: machine === "pc" && id !== null && !!desk.monitors.find((m) => m.id === id)?.ddc,
      seen: true,
      model: p.model,
      serial: byModel ? null : p.serial,
      inches: p.inches,
      // Windows dice por qué cable; CoreGraphics no.
      output: machine === "pc" ? "displayport" : null,
    };
  };
  const plugged = (p: Physical) => !p.pc || ![p.pc, p.mac].some((id) => id && (unplugged.has(id) || unplugged.has(id.toLowerCase())));
  const seenBy = (machine: Machine): CapsMonitor[] => physical.filter(plugged).flatMap((p) => side(p, machine) ?? []);
  const caps = (): Caps => ({
    here: demoCaps(agent, agent === "mac" ? missing : [], seenBy(agent)),
    peer: peer.kind === "paired" ? demoCaps(peerMachine, [], seenBy(peerMachine)) : null,
  });
  /** Los del puesto que alguna máquina ve conectados ahora (como `capabilities::connected`). */
  const connectedIds = () => {
    const c = caps();
    const ids = new Set<string>();
    for (const m of [c.here, c.peer]) for (const x of m?.monitors ?? []) if (x.seen && x.monitor) ids.add(x.monitor);
    return ids;
  };
  const PEER_NAME = peerMachine === "pc" ? "PC" : "Mac";
  const PAIRED: PeerView = { kind: "paired", name: PEER_NAME, link: peerOn ? (clean ? "wifi" : "usb4") : null };
  let peer: PeerView = params.get("peer") === "none" ? { kind: "unpaired" } : PAIRED;
  let pairing: PairingView =
    params.get("pair") === "code" ? { kind: "comparing", code: DEMO_CODE, peer: PEER_NAME, confirmed: false } : { kind: "idle" };
  let pairTimer: number | undefined;
  /** Dónde está el teclado según el asistente; sin respuesta, «sin definir» (T-087). */
  let keyboard: Machine | null = clean ? null : ((params.get("hub") as Machine | null) ?? agent);
  let keyProbe: { armed: boolean; heard: Machine | null } = { armed: false, heard: null };
  const snapshot = (): Snapshot => {
    if (params.get("agent") === "off") {
      return { kind: "unavailable", endpoint: "~/Library/Application Support/Tandem/tandemd.sock", reason: "tandemd no está corriendo: no hay nadie escuchando en la IPC local" };
    }
    const on = connectedIds();
    return {
      kind: "connected",
      agent,
      desk: { monitors: desk.monitors.filter((m) => on.has(m.id)) },
      hidden: desk.monitors.filter((m) => !on.has(m.id)),
      state,
      peer,
      pairing,
      input: {
        server: keyboard ?? agent,
        hub_here: clean ? null : (params.get("hub") ?? agent) === agent,
        session: state.on[peerMachine] && peer.kind === "paired",
        locked: false,
        missing: agent === "mac" ? missing : [],
        problem: null,
        defined: keyboard !== null,
      },
      caps: caps(),
    };
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
  let answer: ((a: ProbeAnswer) => void) | null = null;

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
    // Sin la otra máquina, esta ventana pregunta qué se ve en el monitor.
    const seen = await new Promise<ProbeAnswer>((resolve) => {
      answer = resolve;
      emit({
        kind: "probe",
        event: { type: "show", id: 90, monitor: d.name, input: "hdmi", machine: "mac", ask: { kind: "user_switch" }, timeout_ms: 120000, place: { kind: "question" } },
      });
    });
    answer = null;
    emit({ kind: "probe", event: { type: "hide", id: 90 } });
    if (run.cancelled || seen.kind === "stop") return finish({ type: "finished", outcome: { kind: "cancelled" } });
    if (seen.kind !== "machine" || seen.machine !== "mac") {
      return finish({ type: "finished", outcome: { kind: "failed", reason: "no se confirmó el cambio manual a hdmi desde el menú del monitor" } });
    }
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
      { kind: "found", input: "hdmi", code: 0x05 },
      { kind: "verifying" },
      { kind: "verified", input: "displayport", code: 0x0f },
      { kind: "verified", input: "hdmi", code: 0x05 },
      { kind: "verified", input: "displayport", code: 0x0f },
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
      await sleep(s.step === "power_on" ? STEP_MS * 4 : STEP_MS);
      state = structuredClone(state);
      if (s.step === "power_on") state.on[s.machine] = true;
      if (s.step === "power_off") state.on[s.machine] = false;
      if (s.step === "power_on" && peer.kind === "paired") peer = { ...peer, link: "usb4" };
      if (s.step === "power_off" && peer.kind === "paired") peer = { ...peer, link: null };
      if (s.step === "switch_input") {
        state.owner[s.monitor] = s.to;
        delete state.doubt[s.monitor];
      }
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
        pairTimer = window.setTimeout(() => setPairing({ kind: "comparing", code: DEMO_CODE, peer: PEER_NAME, confirmed: false }), PAIR_FOUND_MS);
        return { kind: "done" };
      case "confirm_pairing":
        if (pairing.kind !== "comparing" || pairing.confirmed) return { kind: "rejected", reason: "no hay ningún código que confirmar" };
        setPairing({ ...pairing, confirmed: true });
        pairTimer = window.setTimeout(() => {
          if (params.get("pair") === "reject") {
            setPairing({ kind: "failed", reason: `se rechazó en la ${PEER_NAME}: no se emparejó` });
            return;
          }
          peer = PAIRED;
          setPairing({ kind: "done", peer: PEER_NAME });
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

  /** Lo que ven los planes: solo los monitores conectados ahora (T-087). */
  const connectedDesk = (): Desk => {
    const on = connectedIds();
    return { monitors: desk.monitors.filter((m) => on.has(m.id)) };
  };
  const pushSnapshot = () => snapshots.forEach((cb) => cb(snapshot()));
  const emitKeyProbe = () => emit({ kind: "key_probe", event: { ...keyProbe } });

  /** Lo conectado ahora a las dos máquinas, juntado por monitor físico (`wizard::propose`). */
  function scan(): MonitorScan {
    const linked = peer.kind === "paired" && peer.link !== null;
    const on = connectedIds();
    const candidates: MonitorCandidate[] = [];
    for (const p of physical.filter(plugged)) {
      const here = side(p, agent);
      const there = linked ? side(p, peerMachine) : null;
      if (!here && !there) continue;
      const mac = agent === "mac" ? here : there;
      const pc = agent === "pc" ? here : there;
      const ids = [pc?.pnp_id, mac?.pnp_id].filter((x): x is string => !!x);
      candidates.push({
        label: p.label,
        pnp_ids: ids,
        here,
        peer: there,
        basis: here && there ? (byModel ? "Model" : "Serial") : "Single",
        wiring: Object.fromEntries([
          ...(here ? [[agent, here.output ?? null]] : []),
          ...(there ? [[peerMachine, there.output ?? null]] : []),
        ]),
        in_desk: desk.monitors.find((m) => ids.some((id) => [m.edid_model, ...(m.edid_aliases ?? [])].includes(id)))?.id ?? null,
        profile: p.profile?.name ?? null,
      });
    }
    return { candidates, peer_known: linked, disconnected: desk.monitors.filter((m) => !on.has(m.id)).length };
  }

  function start(steps: Step[]): ActionResult {
    if (busy) return { kind: "rejected", reason: "ya hay un cambio en curso" };
    // `run` numera la transacción antes de su primer `await`.
    void run(steps);
    return { kind: "accepted", txn };
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
    const cd = connectedDesk();
    const r = start(plan(cd, state, assignment(cd, modeRequest(mode, cd, state))));
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
          return start(plan(connectedDesk(), state, assignment(connectedDesk(), a.mode)));
        case "move_monitor":
          return start(plan(connectedDesk(), state, { ...state.owner, [a.monitor]: a.to }));
        case "set_power":
          if (a.on) return start(state.on[a.machine] ? [] : [{ step: "power_on", machine: a.machine }]);
          return start(plan(connectedDesk(), state, assignment(connectedDesk(), { mode: a.machine === "pc" ? "solo_mac" : "solo_pc" }), a.machine));
        case "set_builtin":
          state = { ...state, builtin_pref: a.display };
          snapshots.forEach((cb) => cb(snapshot()));
          return { kind: "done" };
        case "answer_manual_switch":
          return { kind: "done" };
        case "answer_probe":
          if (answer) {
            answer(a.answer);
            return { kind: "done" };
          }
          emit({ kind: "probe", event: { type: "hide", id: a.id } });
          messages.forEach((cb) => cb(a.answer.kind === "stop" ? "Se cancela la calibración y el monitor vuelve" : "Sigue la calibración con el próximo paso"));
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
          answer?.({ kind: "stop" });
          return { kind: "done" };
        case "use_profile":
          return { kind: "rejected", reason: "en el modo demo ningún monitor tiene perfil en la base" };
        case "set_wiring": {
          if (!a.confirmed) return { kind: "rejected", reason: "cambiar el cableado necesita la confirmación del usuario" };
          if (busy) return { kind: "rejected", reason: "ya hay un cambio en curso" };
          const m = desk.monitors.find((x) => x.id === a.monitor);
          if (!m) return { kind: "rejected", reason: `el monitor \`${a.monitor}\` no está en la configuración` };
          await sleep(200);
          m.wiring = { ...a.wiring };
          snapshots.forEach((cb) => cb(snapshot()));
          return { kind: "done" };
        }
        case "get_setup":
          return { kind: "setup", setup: { config_file: "~/Library/Application Support/Tandem/tandem.toml", generated: true, port: 52000 } };
        case "apply_mode":
          return start(plan(connectedDesk(), state, assignment(connectedDesk(), a.mode), null, true));
        case "scan_monitors":
          await sleep(300);
          return { kind: "scan", scan: scan() };
        case "add_monitors": {
          if (busy) return { kind: "rejected", reason: "ya hay un cambio en curso" };
          const added: MonitorConfig[] = [];
          for (const n of a.monitors) {
            const [first, ...rest] = n.pnp_ids;
            if (!first) return { kind: "rejected", reason: `${n.label}: sin id PnP no hay cómo encontrarlo después` };
            if (desk.monitors.some((m) => [m.edid_model, ...(m.edid_aliases ?? [])].includes(first))) {
              return { kind: "rejected", reason: `${n.label} ya está en el puesto` };
            }
            const known = PHYSICAL.find((p) => [p.pc, p.mac].includes(first));
            added.push({
              id: first.toLowerCase(),
              label: n.label,
              edid_model: first,
              edid_aliases: rest,
              wiring: n.wiring,
              ddc: known?.profile ? { hdmi: known.profile.hdmi, display_port: known.profile.display_port } : null,
            });
          }
          desk = { monitors: [...desk.monitors, ...added] };
          state = structuredClone(state);
          for (const m of added) {
            state.owner[m.id] = "mac";
            state.doubt[m.id] = "unknown";
          }
          pushSnapshot();
          return { kind: "done" };
        }
        case "remove_monitor":
        case "reset_desk": {
          if (busy) return { kind: "rejected", reason: "ya hay un cambio en curso" };
          const gone = a.kind === "reset_desk" ? desk.monitors.map((m) => m.id) : [a.monitor];
          if (a.kind === "remove_monitor" && !desk.monitors.some((m) => m.id === a.monitor)) {
            return { kind: "rejected", reason: `el monitor \`${a.monitor}\` no está en el puesto` };
          }
          desk = { monitors: desk.monitors.filter((m) => !gone.includes(m.id)) };
          state = structuredClone(state);
          for (const id of gone) {
            delete state.owner[id];
            delete state.doubt[id];
          }
          pushSnapshot();
          return { kind: "done" };
        }
        case "mark_manual": {
          const m = desk.monitors.find((x) => x.id === a.monitor);
          if (!m) return { kind: "rejected", reason: `el monitor \`${a.monitor}\` no está en el puesto` };
          m.ddc = null;
          pushSnapshot();
          return { kind: "done" };
        }
        case "key_probe":
          keyProbe = a.on ? { armed: true, heard: null } : { armed: false, heard: null };
          emitKeyProbe();
          // Hace de la otra máquina: si el teclado está en ella, la tecla le llega a ella.
          if (a.on && params.get("kb") === peerMachine) {
            window.setTimeout(() => {
              if (!keyProbe.armed || keyProbe.heard) return;
              keyProbe = { armed: true, heard: peerMachine };
              emitKeyProbe();
            }, 1600);
          }
          return { kind: "done" };
        case "heard_key":
          if (keyProbe.armed && !keyProbe.heard) {
            keyProbe = { armed: true, heard: agent };
            emitKeyProbe();
          }
          return { kind: "done" };
        case "set_keyboard":
          keyboard = a.machine;
          pushSnapshot();
          return { kind: "done" };
        case "ping_peer": {
          await sleep(350);
          if (peer.kind !== "paired" || peer.link === null) return { kind: "rejected", reason: "ningún enlace ve a la otra máquina" };
          return { kind: "pong", link: peer.link, millis: peer.link === "wifi" ? 7 : 1 };
        }
        case "set_shown": {
          if (busy) return { kind: "rejected", reason: "ya hay un cambio en curso" };
          // Como `Plan::follow`: sin entradas, solo los escritorios y la geometría.
          const before = state;
          state = structuredClone(state);
          const steps: Step[] = [];
          for (const [id, shows] of Object.entries(a.shows)) {
            const was = before.doubt[id] !== undefined || before.owner[id] !== shows;
            if (shows === null) {
              state.doubt[id] = "blank";
              continue;
            }
            state.owner[id] = shows;
            delete state.doubt[id];
            if (!was) continue;
            const away: Machine = shows === "mac" ? "pc" : "mac";
            if (state.on[shows]) steps.push({ step: "attach", machine: shows, monitor: id });
            if (state.on[away]) steps.push({ step: "detach", machine: away, monitor: id });
          }
          if (steps.length) steps.push({ step: "update_input_geometry" });
          snapshots.forEach((cb) => cb(snapshot()));
          return start(steps);
        }
      }
    },
  };
}
