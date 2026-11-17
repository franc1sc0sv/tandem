// Agente simulado para desarrollar y sacar capturas en el navegador (`pnpm dev`, `/?demo`).
// Solo se carga en desarrollo: el build de la app nunca lo incluye.
//
// `?demo&agent=off` simula que tandemd no corre; `?demo&pc=on` arranca con la PC encendida;
// `?demo&keys=busy` simula que el sistema no dejó registrar el atajo de la ventana;
// `?demo&setup` abre la guía de permisos del primer arranque (`&setup=fail`: tandemd no se
// pudo instalar); `?demo&peer=none`, un agente sin par emparejado. Emparejar desde ahí simula
// a la otra máquina: aparece el código y, al confirmar, confirma ella también
// (`&pair=reject`: la otra máquina rechaza; `&pair=code`: arranca mostrando el código).
// `?demo&hub=pc`, el hub en la PC; `?demo&perms=missing`, a la Mac le falta el permiso de
// Accesibilidad. `?demo&as=pc` es la ventana de la PC (T-084), con la Mac dormida
// (`&mac=on`: despierta). `?demo&clean` es una instalación limpia: sin par ni teclado dicho,
// y el asistente se abre solo (`&kb=pc`: la tecla de la prueba llega a la PC).
// `?demo&side=right`: el puesto declara la Mac a la derecha de la PC.

import type { Bridge } from "./bridge";
import type {
  Action,
  ActionResult,
  Bindings,
  Caps,
  Desk,
  DeskState,
  Machine,
  MachineCaps,
  Notice,
  PairingView,
  PeerView,
  SetupView,
  ShortcutsView,
  Snapshot,
  Step,
} from "./model";
import { ACTIONS, DEFAULT_BINDINGS, readCombo } from "./shortcuts";

const STEP_MS = 480;
const PAIR_FOUND_MS = 1400;
const PAIR_OTHER_MS = 1200;
const DEMO_CODE = "418027";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Lo que anuncia cada máquina del puesto real: la PC se apaga sola, la Mac no. */
function demoCaps(machine: Machine, missing: MachineCaps["input"]["missing"]): MachineCaps {
  const pc = machine === "b";
  return {
    machine,
    name: pc ? "ESCRITORIO" : "MacBook-Pro",
    os: pc ? "windows" : "macos",
    color: pc ? "#a99be8" : "#7aa7e6",
    hub: null,
    input: {
      capture: missing.length === 0,
      inject: missing.length === 0,
      missing,
      process: pc
        ? { exe: "C:\\Program Files\\Tandem\\tandemd.exe", pid: 5120, started: 1_760_000_000 }
        : { exe: "/Applications/Tandem.app/Contents/MacOS/tandemd", pid: 4242, started: 1_760_000_000 },
    },
    power: { wake_peer: true, wakeable: true, power_off: pc },
    services: [],
  };
}

export function demoBridge(query: string): Bridge {
  const params = new URLSearchParams(query);
  // La ventana de qué máquina es: la Mac por omisión, la PC con `&as=pc`.
  const agent: Machine = params.get("as") === "b" ? "b" : "a";
  const peerMachine: Machine = agent === "a" ? "b" : "a";
  const clean = params.has("clean");
  const peerOn = params.get(peerMachine) === "on";
  const desk: Desk = { screens: [], arrangement: { a_side: params.get("side") === "right" ? "right" : "left", offset: 0 } };
  let state: DeskState = { on: { [agent]: true, [peerMachine]: peerOn } as Record<Machine, boolean> };
  const missing: MachineCaps["input"]["missing"] = params.get("perms") === "missing" ? ["accessibility", "input_monitoring"] : [];
  const PEER_NAME = peerMachine === "b" ? "PC" : "Mac";
  const PAIRED: PeerView = { kind: "paired", name: PEER_NAME, link: peerOn ? "usb4" : null };
  let peer: PeerView = params.get("peer") === "none" || clean ? { kind: "unpaired" } : PAIRED;
  let pairing: PairingView =
    params.get("pair") === "code" ? { kind: "comparing", code: DEMO_CODE, peer: PEER_NAME, confirmed: false } : { kind: "idle" };
  let pairTimer: number | undefined;
  /** Dónde está el teclado según el asistente; sin respuesta, «sin definir» (T-087). */
  let keyboard: Machine | null = clean ? null : ((params.get("hub") as Machine | null) ?? agent);
  let keyProbe: { armed: boolean; heard: Machine | null } = { armed: false, heard: null };
  const caps = (): Caps => ({
    here: demoCaps(agent, agent === "a" ? missing : []),
    peer: peer.kind === "paired" ? demoCaps(peerMachine, []) : null,
  });
  const snapshot = (): Snapshot => {
    if (params.get("agent") === "off") {
      return { kind: "unavailable", endpoint: "~/Library/Application Support/Tandem/tandemd.sock", reason: "tandemd no está corriendo: no hay nadie escuchando en la IPC local" };
    }
    return {
      kind: "connected",
      agent,
      desk,
      state,
      peer,
      pairing,
      input: {
        server: keyboard ?? agent,
        hub_here: clean ? null : (params.get("hub") ?? agent) === agent,
        session: state.on[peerMachine] && peer.kind === "paired",
        locked: false,
        missing: agent === "a" ? missing : [],
        problem: null,
        defined: keyboard !== null,
      },
      caps: caps(),
    };
  };
  let keys: ShortcutsView = {
    bindings: { ...DEFAULT_BINDINGS },
    failed: params.get("keys") === "busy" ? { toggle_window: "el sistema no dejó registrar Ctrl+Alt+Digit0 (¿lo usa otra app?)" } : {},
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
  const pushSnapshot = () => snapshots.forEach((cb) => cb(snapshot()));
  const emitKeyProbe = () => emit({ kind: "key_probe", event: { ...keyProbe } });
  let txn = 0;
  let busy = false;

  async function run(steps: Step[]) {
    const id = ++txn;
    busy = true;
    emit({ kind: "txn", event: { type: "started", txn: id, steps } });
    for (const [index, s] of steps.entries()) {
      emit({ kind: "txn", event: { type: "step_started", txn: id, index } });
      await sleep(s.step === "power_on" ? STEP_MS * 4 : STEP_MS);
      state = { on: { ...state.on, [s.machine]: s.step === "power_on" } };
      if (peer.kind === "paired") peer = { ...peer, link: s.step === "power_on" ? "usb4" : null };
      emit({ kind: "txn", event: { type: "step_done", txn: id, index } });
    }
    busy = false;
    pushSnapshot();
    emit({ kind: "txn", event: { type: "committed", txn: id, state } });
  }

  function start(steps: Step[]): ActionResult {
    if (busy) return { kind: "rejected", reason: "ya hay un cambio en curso" };
    // `run` numera la transacción antes de su primer `await`.
    void run(steps);
    return { kind: "accepted", txn };
  }

  function setPairing(next: PairingView) {
    pairing = next;
    pushSnapshot();
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
          peer = { kind: "paired", name: PEER_NAME, link: "usb4" };
          state = { on: { ...state.on, [peerMachine]: true } };
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

  // Hace de sistema: el atajo «global» se atiende aquí, como lo haría el núcleo.
  addEventListener("keydown", (e) => {
    const c = readCombo(e);
    if (c.kind !== "ok") return;
    const action = ACTIONS.find((a) => keys.bindings[a] === c.accel && !(a in keys.failed));
    if (!action) return;
    e.preventDefault();
    messages.forEach((cb) => cb("Aquí la ventana se escondería (atajo global)"));
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
    restartAgent: async () => {
      messages.forEach((cb) => cb("Aquí se reiniciaría tandemd (launchctl kickstart -k)"));
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
    },
    act: async (a: Action): Promise<ActionResult> => {
      switch (a.kind) {
        case "set_power": {
          const on = state.on[a.machine];
          if (on === a.on) return start([]);
          return start([{ step: a.on ? "power_on" : "power_off", machine: a.machine }]);
        }
        case "start_pairing":
        case "confirm_pairing":
        case "cancel_pairing":
        case "unpair":
          return pairAction(a.kind);
        case "get_setup":
          return { kind: "setup", setup: { config_file: "~/Library/Application Support/Tandem/tandem.toml", generated: true, port: 52000 } };
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
        case "request_permissions":
          messages.forEach((cb) => cb("Aquí macOS mostraría su diálogo de permisos para tandemd"));
          return { kind: "done" };
        case "ping_peer": {
          await sleep(350);
          if (peer.kind !== "paired" || peer.link === null) return { kind: "rejected", reason: "ningún enlace ve a la otra máquina" };
          return { kind: "pong", link: peer.link, millis: peer.link === "wifi" ? 7 : 1 };
        }
      }
    },
  };
}
