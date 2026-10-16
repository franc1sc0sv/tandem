import "./styles.css";

import { bridgeFailed, tauriBridge, type Bridge } from "./bridge";
import { INPUTS, SHORT, actions as rowActions, advance as advanceCalibration, badge, headline, hint, labelOf, laneText, unbound, type Calibration } from "./addmonitor";
import type { Action, Bindings, BuiltinDisplay, DisplayView, Machine, ManualView, Notice, PairingView, Permission, ProbeShow, SetupView, ShortcutAction, ShortcutsView, Snapshot } from "./model";
import { isActive, needsDismiss, pairLine, pairScreen, shouldOpen } from "./pairing";
import { PORT_NAME, nextQuestion, questionText } from "./probe";
import {
  MACHINE_NAME,
  MACHINES,
  PORT,
  advance,
  builtinOf,
  groupStatus,
  groupSteps,
  isRunning,
  modeOf,
  modeRequest,
  monitorsOf,
  other,
  pcPower,
  peerPill,
  roleChip,
  type ModeKey,
  type MonitorView,
  type Progress,
  type RoleChip,
} from "./scene";
import {
  ACTION_LABEL,
  ACTION_OF_MODE,
  ACTIONS,
  DEFAULT_BINDINGS,
  ariaKeys,
  duplicateOf,
  formatAccel,
  isMacPlatform,
  localAction,
  readCombo,
  windowCommand,
} from "./shortcuts";
import { PERMISSION_INFO, back as setupBack, isFirst, isLast, next as setupNext, screensOf, start as setupStart, type Wizard } from "./setup";

// Cuánto queda a la vista el progreso ya terminado y un mensaje suelto.
const DONE_LINGER_MS = 800;
const MESSAGE_MS = 6000;

const $ = <T extends Element = HTMLElement>(sel: string, root: ParentNode = document) => root.querySelector(sel) as T;
const $$ = <T extends Element = HTMLElement>(sel: string, root: ParentNode = document) => [...root.querySelectorAll(sel)] as T[];

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

let bridge: Bridge | null = null;
let snap: Snapshot = { kind: "connecting" };
let progress: Progress | null = null;
let manual: Extract<ManualView, { type: "requested" }> | null = null;
/** La pregunta de respaldo de una calibración sin la otra máquina (T-078). */
let question: ProbeShow | null = null;
let message: { text: string; sticky: boolean } | null = null;
let messageTimer: number | undefined;
let lastOwner: Record<string, Machine> | null = null;
let keys: ShortcutsView | null = null;
let lastPairing: PairingView | null = null;
const MAC = isMacPlatform();

// ---------------------------------------------------------------------------------------
// Estado

function setSnapshot(next: Snapshot) {
  const pairingBefore = lastPairing;
  lastPairing = next.kind === "connected" ? next.pairing : null;
  const before = lastOwner;
  const after = next.kind === "connected" ? { ...next.state.owner } : null;
  const moved = before && after ? Object.keys(after).filter((id) => before[id] !== after[id]) : [];
  snap = next;
  lastOwner = after;
  if (moved.length && document.startViewTransition) {
    document.startViewTransition(() => render(moved));
  } else {
    render(moved);
  }
  syncPair(pairingBefore);
}

function onNotice(n: Notice) {
  if (n.kind === "txn") {
    progress = advance(progress, n.event);
    const p = progress;
    if (p && p.outcome.kind === "committed") {
      window.setTimeout(() => {
        if (progress === p) {
          progress = null;
          renderToast();
        }
      }, DONE_LINGER_MS);
    }
    if (p && p.outcome.kind === "rolled_back") {
      showMessage(`No se pudo: ${p.outcome.error}. Todo volvió como estaba.`);
      progress = null;
    }
    if (p && p.outcome.kind === "inconsistent") {
      showMessage(`No se pudo deshacer todo: ${p.outcome.error}. Revisa los monitores.`, true);
      progress = null;
    }
    // El botón de encendido y el bloqueo de controles dependen del progreso; los
    // monitores no se vuelven a dibujar para no cortar su animación.
    renderControls();
    renderToast();
  } else if (n.kind === "power") {
    // Fijo: los monitores que quedaron en la PC siguen en negro hasta encenderla.
    showMessage(n.reason, n.stranded.length > 0);
  } else if (n.kind === "probe") {
    // La sonda a pantalla completa tiene su propia ventana; aquí solo la pregunta.
    const before = question;
    question = nextQuestion(question, n.event);
    if (question !== before) {
      renderToast();
      if (addmonDialog().open) renderAddmon();
    }
  } else if (n.kind === "calibration") {
    calibration = advanceCalibration(calibration, n.event);
    // Una calibración que arrancó (desde esta u otra ventana) se sigue en el diálogo.
    if (n.event.type === "started" && !addmonDialog().open) addmonDialog().showModal();
    renderAddmon();
  } else if (n.event.type === "requested") {
    manual = n.event;
    renderToast();
  } else if (manual && manual.id === n.event.id) {
    manual = null;
    renderToast();
  }
}

function showMessage(text: string, sticky = false) {
  message = { text, sticky };
  window.clearTimeout(messageTimer);
  if (!sticky) {
    messageTimer = window.setTimeout(() => {
      message = null;
      renderToast();
    }, MESSAGE_MS);
  }
  renderToast();
}

/** Pide `action`; si no se puede, devuelve por qué (y, salvo `quiet`, lo muestra en el aviso). */
async function act(action: Action, quiet = false): Promise<string | null> {
  if (!bridge) return null;
  const r = await bridge.act(action).catch((e: unknown) => ({ kind: "unavailable" as const, reason: String(e) }));
  const why = r.kind === "rejected" ? r.reason : r.kind === "unavailable" ? `Sin agente: ${r.reason}` : null;
  if (why && !quiet) showMessage(why);
  return why;
}

// ---------------------------------------------------------------------------------------
// Dibujo

const ICON_KB = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/></svg>`;
const ICON_CURSOR = `<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M5 3l14 7-6 2-2 6z"/></svg>`;
const ICON_WARN = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M12 3l9.5 17h-19z"/><path d="M12 10v4M12 17h.01"/></svg>`;
const ROLE_ICON: Record<RoleChip["icon"], string> = { keyboard: ICON_KB, cursor: ICON_CURSOR, warn: ICON_WARN };
const ICON_EMPTY = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M9 20h6M12 16v4"/></svg>`;
const LAP_MIRROR = `<g transform="translate(96 36)" stroke="#9fb6d8" stroke-width="2.4" fill="none" stroke-linecap="round"><rect x="0" y="0" width="12" height="10" rx="2"/><rect x="16" y="0" width="12" height="10" rx="2"/><path d="M6 17h16"/></g>`;
const LAP_OFF = `<g transform="translate(100 38)" stroke="#3a3d45" stroke-width="2.4" fill="none" stroke-linecap="round"><path d="M0 0l20 20M20 0 0 20"/></g>`;

function viewName(id: string) {
  return `monitor-${id.replace(/[^a-zA-Z0-9_-]/g, "_")}`;
}

function monitorEl(m: MonitorView, busy: boolean) {
  const node = el("div", `monitor ${m.owner} ${m.wide ? "wide" : "std"}${busy ? " busy" : ""}`);
  node.dataset.id = m.id;
  node.style.viewTransitionName = viewName(m.id);
  node.innerHTML = `<div class="screen"><div class="wall"></div></div><div class="stand"></div><div class="base"></div>`;
  const label = el("div", "mlabel");
  label.append(el("b", undefined, m.label));
  if (m.port) label.append(el("span", "port", m.port));
  node.append(label);

  const to = other(m.owner);
  if (m.movable) {
    node.draggable = true;
    node.tabIndex = 0;
    node.setAttribute("role", "button");
    node.setAttribute("aria-label", `${m.label} en ${MACHINE_NAME[m.owner]}. Enter: pasarlo a la ${MACHINE_NAME[to]}`);
    node.addEventListener("dragstart", (e) => e.dataTransfer?.setData("text/plain", m.id));
    node.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        void act({ kind: "move_monitor", monitor: m.id, to });
      }
    });
  } else {
    node.classList.add("fixed");
    node.setAttribute("aria-label", `${m.label} en ${MACHINE_NAME[m.owner]}`);
  }
  return node;
}

function renderAgentPill() {
  const text = $("#agent-text");
  const dot = $("#agent-dot");
  if (snap.kind === "connected") {
    text.textContent = `tandemd · ${MACHINE_NAME[snap.agent]}`;
    dot.classList.remove("warn");
  } else {
    text.textContent = snap.kind === "connecting" ? "Conectando…" : "Sin agente";
    dot.classList.add("warn");
  }
}

function renderUnavailable() {
  const stage = $("#stage");
  if (snap.kind !== "unavailable") {
    stage.replaceChildren();
    return;
  }
  const card = el("section", "card");
  card.append(el("h1", "title", "No se pudo hablar con tandemd"), el("p", "reason", snap.reason));
  if (snap.endpoint) {
    const where = el("p", "endpoint", "Buscado en ");
    where.append(el("code", undefined, snap.endpoint));
    card.append(where);
  }
  const retry = el("button", "pill btn", "Reintentar");
  retry.type = "button";
  retry.addEventListener("click", () => void bridge?.retry());
  card.append(retry);
  stage.replaceChildren(card);
}

function renderDesks(moved: string[]) {
  if (snap.kind !== "connected") return;
  const { desk, state } = snap;
  for (const who of MACHINES) {
    const deskEl = $(`.desk[data-owner="${who}"]`);
    const mine = monitorsOf(desk, state, who);
    deskEl.replaceChildren(...mine.map((m) => monitorEl(m, moved.includes(m.id))));
    if (!mine.length) {
      const empty = el("div", "empty");
      empty.innerHTML = ICON_EMPTY;
      empty.append(who === "pc" && !state.pc_on ? "PC apagada" : "Arrastra un monitor aquí");
      deskEl.append(empty);
    }
  }
}

/** Lo que cambia también con el avance de una transacción. */
function renderControls() {
  if (snap.kind !== "connected") return;
  const { desk, state, agent } = snap;
  const running = isRunning(progress);
  const power = pcPower(state, progress);

  for (const who of MACHINES) {
    $(`#state-${who}`).textContent =
      who === "pc" ? { on: "encendida", boot: "arrancando…", off: "apagada" }[power] : agent === who ? "esta máquina" : "";
    const role = $(`#role-${who}`);
    const chip = roleChip(snap.input, agent, who, power === "on");
    if (chip) {
      role.className = `role ${chip.kind}`;
      role.innerHTML = `${ROLE_ICON[chip.icon]} ${chip.text}`;
      role.title = chip.title;
      role.hidden = false;
    } else {
      role.hidden = true;
    }
  }
  $(".side.pc").classList.toggle("asleep", power !== "on");

  const pw = $<HTMLButtonElement>("#power");
  pw.className = `power ${power === "off" ? "" : power}`.trim();
  pw.setAttribute("aria-label", power === "on" ? "Apagar la PC" : power === "boot" ? "La PC está arrancando" : "Encender la PC");
  pw.title = pw.getAttribute("aria-label") ?? "";
  pw.setAttribute("aria-disabled", String(running));

  const bi = builtinOf(desk, state);
  for (const b of $$<HTMLButtonElement>("#builtin-seg button")) {
    const v = b.dataset.v;
    if (v === "gather") continue;
    const sel = v === bi.effective;
    b.classList.toggle("sel", sel);
    b.setAttribute("aria-checked", String(sel));
    b.disabled = running || (bi.forced && v !== "extend");
  }
  $("#lap-screen").setAttribute("fill", bi.effective === "off" ? "#111214" : "#2d3e55");
  $("#lap-icon").innerHTML = bi.effective === "mirror" ? LAP_MIRROR : bi.effective === "off" ? LAP_OFF : "";
  $("#builtin-note").textContent = bi.note;

  const mode = modeOf(desk, state);
  for (const m of $$<HTMLButtonElement>(".mode")) {
    const sel = m.dataset.mode === mode;
    m.classList.toggle("sel", sel);
    m.setAttribute("aria-pressed", String(sel));
  }

  const linkPill = $("#link-pill");
  const pill = peerPill(snap.peer);
  linkPill.hidden = false;
  $("#link-text").textContent = pill.text;
  $("#link-dot").classList.toggle("warn", !pill.ok);
  $("#link-act").hidden = snap.peer.kind === "paired";
  linkPill.title = snap.peer.kind === "paired" ? `${pill.title}. Clic: ver el par` : `${pill.title}. Clic: emparejar`;
}

function renderToast() {
  const t = $("#toast");
  const parts: Node[] = [];
  if (question) {
    const q = question;
    const text = questionText(q);
    parts.push(el("span", "msg", text.text));
    for (const c of text.choices) {
      const b = el("button", "act", c.label);
      b.addEventListener("click", () => void act({ kind: "answer_probe", id: q.id, answer: c.answer }));
      parts.push(b);
    }
  } else if (manual) {
    const m = manual;
    parts.push(el("span", "msg", `Cambia el ${m.label} a ${PORT[m.input]} con los botones del monitor`));
    const done = el("button", "act", "Listo");
    const cant = el("button", "act", "No puedo");
    done.addEventListener("click", () => void act({ kind: "answer_manual_switch", id: m.id, done: true }));
    cant.addEventListener("click", () => void act({ kind: "answer_manual_switch", id: m.id, done: false }));
    parts.push(done, cant);
  } else if (progress && snap.kind === "connected") {
    const p = progress;
    groupSteps(snap.desk, p.steps).forEach((g, i) => {
      if (i) parts.push(el("span", "sep"));
      const s = el("span", `step ${groupStatus(p, g)}`);
      s.append(el("i"), g.label);
      parts.push(s);
    });
    if (p.outcome.kind === "rolling_back") {
      parts.push(el("span", "break"), el("span", "msg warn", `Deshaciendo: ${p.outcome.error}`));
    }
  } else if (message) {
    parts.push(el("span", "msg warn", message.text));
    if (message.sticky) {
      const close = el("button", "act", "Cerrar");
      close.addEventListener("click", () => {
        message = null;
        renderToast();
      });
      parts.push(close);
    }
  }
  t.replaceChildren(...parts);
  t.classList.toggle("show", parts.length > 0);
}

function render(moved: string[]) {
  renderAgentPill();
  const connected = snap.kind === "connected";
  $("#scene").hidden = !connected;
  $("#bottom").hidden = !connected;
  $("#stage").hidden = connected;
  $("#monitors-btn").hidden = !connected;
  if (!connected) $("#link-pill").hidden = true;
  renderUnavailable();
  renderDesks(moved);
  renderControls();
  renderToast();
}

/** El atajo de cada tarjeta de modo, el que esté configurado. */
function renderModeKeys() {
  for (const m of $$<HTMLButtonElement>(".mode")) {
    const accel = keys?.bindings[ACTION_OF_MODE[m.dataset.mode as ModeKey]] ?? null;
    $(".k", m).textContent = formatAccel(accel, MAC);
    if (accel) m.setAttribute("aria-keyshortcuts", ariaKeys(accel));
    else m.removeAttribute("aria-keyshortcuts");
  }
}

// ---------------------------------------------------------------------------------------
// Atajos globales: el diálogo para cambiarlos

let draft: Bindings = { ...DEFAULT_BINDINGS };
let recording: ShortcutAction | null = null;
const rowError: Partial<Record<ShortcutAction, string>> = {};
let saveError: string | null = null;

function openKeys() {
  if (!keys) return;
  draft = { ...keys.bindings };
  recording = null;
  saveError = null;
  for (const a of ACTIONS) delete rowError[a];
  renderKeysDialog();
  $<HTMLDialogElement>("#keys").showModal();
}

function renderKeysDialog() {
  const list = $("#keys-list");
  const rows = ACTIONS.map((a) => {
    const row = el("div", "krow");
    row.append(el("span", "klabel", ACTION_LABEL[a]));
    const combo = el("button", `combo${recording === a ? " rec" : ""}`);
    combo.type = "button";
    combo.textContent = recording === a ? "Pulsa la combinación…" : formatAccel(draft[a], MAC) || "sin atajo";
    if (!draft[a] && recording !== a) combo.classList.add("none");
    combo.setAttribute("aria-label", `${ACTION_LABEL[a]}: ${combo.textContent}. Cambiar`);
    combo.dataset.action = a;
    combo.addEventListener("click", () => {
      recording = recording === a ? null : a;
      delete rowError[a];
      renderKeysDialog();
    });
    const clear = el("button", "kclear", "Quitar");
    clear.type = "button";
    clear.disabled = !draft[a];
    clear.setAttribute("aria-label", `Quitar el atajo de ${ACTION_LABEL[a]}`);
    clear.addEventListener("click", () => {
      draft = { ...draft, [a]: null };
      if (recording === a) recording = null;
      renderKeysDialog();
    });
    row.append(combo, clear);
    // Un atajo que el sistema no dejó registrar solo funciona con la ventana al frente.
    const failed = keys && draft[a] === keys.bindings[a] ? keys.failed[a] : undefined;
    const note = rowError[a] ?? (failed ? `${failed}. Funciona solo con Tandem al frente.` : null);
    if (note) row.append(el("span", "knote", note));
    return row;
  });
  const focused = (document.activeElement as HTMLElement | null)?.dataset?.action;
  list.replaceChildren(...rows);
  // Redibujar quita el foco: devolverlo a la fila que se estaba tocando.
  const again = recording ?? focused;
  if (again) $<HTMLButtonElement>(`#keys-list .combo[data-action="${again}"]`).focus();
  const warn = $("#keys-warning");
  const text = saveError ?? keys?.warning ?? null;
  warn.textContent = text ?? "";
  warn.hidden = !text;
  const file = $("#keys-file");
  file.textContent = keys?.file ? `Se guardan en ${keys.file}` : "";
}

function onKeysKeydown(e: KeyboardEvent) {
  if (!recording || !$<HTMLDialogElement>("#keys").open) return;
  e.preventDefault();
  e.stopPropagation();
  const a = recording;
  if (e.code === "Escape" && !e.ctrlKey && !e.altKey && !e.metaKey) {
    recording = null;
    renderKeysDialog();
    return;
  }
  const c = readCombo(e);
  if (c.kind === "partial") return;
  if (c.kind === "invalid") {
    rowError[a] = c.reason;
  } else {
    const dup = duplicateOf(draft, a, c.accel);
    if (dup) {
      rowError[a] = `ya lo usa «${ACTION_LABEL[dup]}»`;
    } else {
      draft = { ...draft, [a]: c.accel };
      delete rowError[a];
      recording = null;
    }
  }
  renderKeysDialog();
}

async function saveKeys() {
  if (!bridge) return;
  try {
    keys = await bridge.setShortcuts(draft);
    saveError = null;
    renderModeKeys();
    // Si alguno no se pudo registrar, el diálogo queda abierto para que se vea por qué.
    if (Object.keys(keys.failed).length === 0) $<HTMLDialogElement>("#keys").close();
    else renderKeysDialog();
  } catch (e) {
    saveError = `No se guardó: ${e instanceof Error ? e.message : String(e)}`;
    renderKeysDialog();
  }
}

function wireKeys() {
  const dialog = $<HTMLDialogElement>("#keys");
  $("#keys-btn").addEventListener("click", openKeys);
  // En la ventana y en captura: grabar no depende de dónde quedó el foco.
  addEventListener("keydown", onKeysKeydown, true);
  // Escape mientras se graba cancela la grabación, no el diálogo.
  dialog.addEventListener("cancel", (e) => {
    if (recording) e.preventDefault();
  });
  $("#keys-reset").addEventListener("click", () => {
    draft = { ...DEFAULT_BINDINGS };
    recording = null;
    for (const a of ACTIONS) delete rowError[a];
    renderKeysDialog();
  });
  $("#keys-cancel").addEventListener("click", () => dialog.close());
  $("#keys-save").addEventListener("click", () => void saveKeys());
}

// ---------------------------------------------------------------------------------------
// Primer arranque (macOS): la guía de permisos

let setupView: SetupView | null = null;
let wizard: Wizard | null = null;
let setupError: string | null = null;

const ICON_LOGO = `<svg viewBox="0 0 32 32" fill="none" aria-hidden="true"><circle cx="11.5" cy="16" r="8" stroke="#7aa7e6" stroke-width="2.4"/><circle cx="20.5" cy="16" r="8" stroke="#a99be8" stroke-width="2.4"/></svg>`;
const ICON_PERMISSION: Record<Permission, string> = {
  accessibility: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="4.5" r="1.8"/><path d="M5 8.5l7 1.5 7-1.5M12 10v4.5M12 14.5 9 21M12 14.5l3 6.5"/></svg>`,
  input_monitoring: ICON_KB,
  local_network: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><rect x="9" y="3" width="6" height="5" rx="1.2"/><rect x="3" y="16" width="6" height="5" rx="1.2"/><rect x="15" y="16" width="6" height="5" rx="1.2"/><path d="M12 8v4M6 16v-4h12v4"/></svg>`,
};

function openSetup() {
  if (!setupView?.available) return;
  wizard = setupStart(setupView);
  setupError = null;
  renderSetup();
  const d = $<HTMLDialogElement>("#setup");
  if (!d.open) d.showModal();
}

function renderSetup() {
  if (!setupView || !wizard) return;
  const w = wizard;
  const screen = screensOf(setupView)[w.index];
  const dots = Array.from({ length: w.total }, (_, i) => el("span", i <= w.index ? "on" : undefined));
  $("#setup-dots").replaceChildren(...dots);

  const body: Node[] = [];
  if (!screen || screen.kind === "intro") {
    const logo = el("div", "slogo");
    logo.innerHTML = ICON_LOGO;
    body.push(logo, el("h2", undefined, "Tandem en esta Mac"));
    const title = body[1] as HTMLElement;
    title.id = "setup-title";
    body.push(el("p", undefined, "tandemd es el agente que habla con la PC: arranca solo al iniciar sesión. Para llevar el teclado, el mouse y los monitores de una máquina a la otra, macOS le pide tres permisos."));
    const agent = setupView.agent;
    if (agent) {
      const line = el("div", "sagent");
      line.append(el("span", `dot${agent.ok ? "" : " warn"}`), agent.text);
      body.push(line);
    }
    const list = el("div", "slist");
    for (const p of setupView.permissions) {
      const row = el("div");
      row.innerHTML = ICON_PERMISSION[p];
      row.append(PERMISSION_INFO[p].title);
      list.append(row);
    }
    body.push(list);
  } else {
    const p = screen.permission;
    const info = PERMISSION_INFO[p];
    const tile = el("div", "stile");
    tile.innerHTML = ICON_PERMISSION[p];
    const title = el("h2", undefined, info.title);
    title.id = "setup-title";
    body.push(tile, title, el("p", undefined, info.why), el("p", "how", info.how));
    const open = el("button", "kbtn primary sopen", "Abrir Ajustes del Sistema");
    open.type = "button";
    open.addEventListener("click", () => {
      setupError = null;
      bridge?.openSettings(p).catch((e: unknown) => {
        setupError = `No se pudo abrir Ajustes: ${e instanceof Error ? e.message : String(e)}`;
        renderSetup();
      });
    });
    body.push(open);
  }
  if (setupError) body.push(el("p", "serr", setupError));
  $("#setup-body").replaceChildren(...body);

  $<HTMLButtonElement>("#setup-back").hidden = isFirst(w);
  $("#setup-next").textContent = isLast(w) ? "Terminar" : isFirst(w) ? "Empezar" : "Siguiente";
}

async function finishSetup() {
  if (!bridge) return;
  try {
    setupView = await bridge.finishSetup();
    $<HTMLDialogElement>("#setup").close();
  } catch (e) {
    setupError = `No se guardó: ${e instanceof Error ? e.message : String(e)}`;
    renderSetup();
  }
}

function wireSetup() {
  $("#setup-btn").addEventListener("click", openSetup);
  $("#setup-later").addEventListener("click", () => $<HTMLDialogElement>("#setup").close());
  $("#setup-back").addEventListener("click", () => {
    if (!wizard) return;
    wizard = setupBack(wizard);
    setupError = null;
    renderSetup();
  });
  $("#setup-next").addEventListener("click", () => {
    if (!wizard) return;
    if (isLast(wizard)) {
      void finishSetup();
      return;
    }
    wizard = setupNext(wizard);
    setupError = null;
    renderSetup();
  });
}

// ---------------------------------------------------------------------------------------
// Emparejamiento (DESIGN §9): el mismo código en las dos pantallas

let askUnpair = false;
/** Lo que el agente rechazó desde el diálogo: el aviso de abajo queda tapado por él. */
let pairError: string | null = null;
const ICON_CHECK = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m5 12 5 5 9-10"/></svg>`;

function pairDialog() {
  return $<HTMLDialogElement>("#pair");
}

function openPair() {
  if (snap.kind !== "connected") return;
  askUnpair = false;
  pairError = null;
  renderPair();
  if (!pairDialog().open) pairDialog().showModal();
}

/** Abre el diálogo si el emparejamiento empezó (o mostró su código) y lo redibuja. */
function syncPair(before: PairingView | null) {
  const d = pairDialog();
  if (snap.kind !== "connected") {
    if (d.open) d.close();
    return;
  }
  if (!d.open && shouldOpen(before, snap.pairing)) {
    openPair();
    return;
  }
  if (d.open) renderPair();
}

function kbtn(text: string, onClick: () => void, cls = "") {
  const b = el("button", `kbtn ${cls}`.trim(), text);
  b.type = "button";
  b.addEventListener("click", onClick);
  return b;
}

function renderPair() {
  if (snap.kind !== "connected") return;
  const { agent, peer, pairing } = snap;
  const screen = pairScreen(agent, peer, pairing, askUnpair);
  $("#pair-scene").dataset.line = pairLine(screen);
  for (const tile of $$("#pair-scene .ptile")) {
    $("em", tile).textContent = tile.dataset.m === agent ? "esta máquina" : "";
  }

  const body: Node[] = [];
  const foot: Node[] = [];
  const title = (text: string) => {
    const h = el("h2", undefined, text);
    h.id = "pair-title";
    body.push(h);
  };
  const close = (text = "Cerrar") => kbtn(text, () => pairDialog().close());
  switch (screen.kind) {
    case "intro":
      title(`Emparejar con la ${screen.other}`);
      body.push(el("p", undefined, `Se hace una sola vez. Pulsa «Emparejar» aquí y en la ${screen.other}, y comprueba que las dos pantallas muestran el mismo código.`));
      foot.push(close(), el("span", "spacer"), kbtn("Emparejar", () => void pairAct({ kind: "start_pairing" }), "primary"));
      break;
    case "searching":
      title(`Buscando la ${screen.other}…`);
      body.push(el("p", undefined, screen.hint), el("p", undefined, "Las dos tienen que estar en el mismo cable o en la misma red."));
      foot.push(el("span", "spacer"), close("Cancelar"));
      break;
    case "compare": {
      title(`¿La ${screen.peer} muestra este código?`);
      const code = el("div", "pcode");
      code.setAttribute("aria-label", `Código ${screen.digits.join(" ")}`);
      code.append(el("span", undefined, screen.digits[0]), el("span", undefined, screen.digits[1]));
      body.push(code);
      if (screen.confirmed) {
        body.push(el("p", "pok", `Confirmado aquí. Falta confirmar en la ${screen.peer}.`));
      } else {
        body.push(el("p", undefined, "Si es el mismo, confírmalo en las dos máquinas. Si no, recházalo: otra máquina podría estar haciéndose pasar por ella."));
      }
      const yes = kbtn("Coinciden", () => void pairAct({ kind: "confirm_pairing" }), "primary");
      yes.disabled = screen.confirmed;
      foot.push(close("No coinciden"), el("span", "spacer"), yes);
      break;
    }
    case "done": {
      title(`Emparejado con ${screen.peer}`);
      const ok = el("div", "stile");
      ok.innerHTML = ICON_CHECK;
      ok.style.color = "var(--ok)";
      body.push(ok, el("p", undefined, "Desde ahora se conectan solas, cifradas y por el mejor enlace."));
      foot.push(el("span", "spacer"), close("Listo"));
      break;
    }
    case "failed":
      title("No se emparejó");
      body.push(el("p", "pwarn", capitalize(screen.reason)), el("p", undefined, "No se guardó nada en ninguna de las dos máquinas."));
      foot.push(close(), el("span", "spacer"), kbtn("Reintentar", () => void pairAct({ kind: "start_pairing" }), "primary"));
      break;
    case "paired":
      title(`Emparejado con ${screen.name}`);
      body.push(el("p", undefined, `Esta máquina solo acepta a la ${screen.name}: toda la conexión entre las dos va cifrada.`));
      foot.push(
        kbtn("Desemparejar…", () => {
          askUnpair = true;
          renderPair();
        }, "danger"),
        el("span", "spacer"),
        close(),
      );
      break;
    case "unpair":
      title(`¿Desemparejar la ${screen.name}?`);
      body.push(el("p", undefined, `Esta máquina olvida su llave y deja de conectarse con ella. Para volver a usarlas juntas hay que emparejarlas de nuevo; desemparéjala también en la ${screen.name}.`));
      foot.push(
        kbtn("Cancelar", () => {
          askUnpair = false;
          renderPair();
        }),
        el("span", "spacer"),
        kbtn("Desemparejar", () => void unpair(), "danger"),
      );
      break;
  }
  if (pairError) body.push(el("p", "pwarn", capitalize(pairError)));
  $("#pair-body").replaceChildren(...body);
  $("#pair-foot").replaceChildren(...foot);
}

/** Un pedido desde el diálogo: el motivo de un rechazo se muestra dentro de él. */
async function pairAct(action: Action) {
  pairError = await act(action, true);
  renderPair();
}

function capitalize(text: string) {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

async function unpair() {
  askUnpair = false;
  pairError = await act({ kind: "unpair" }, true);
  if (pairError) renderPair();
  else if (pairDialog().open) pairDialog().close();
}

function wirePair() {
  $("#link-pill").addEventListener("click", openPair);
  // Cerrar (con un botón o Escape) cancela lo que esté vivo y descarta un resultado.
  pairDialog().addEventListener("close", () => {
    askUnpair = false;
    pairError = null;
    if (snap.kind === "connected" && (isActive(snap.pairing) || needsDismiss(snap.pairing))) {
      void act({ kind: "cancel_pairing" });
    }
  });
}

// ---------------------------------------------------------------------------------------
// «Agregar monitor» (T-080): los monitores que ve la PC y la calibración de los que no
// tienen perfil

let displays: DisplayView[] | null = null;
let displaysError: string | null = null;
let calibration: Calibration | null = null;
let addmonError: string | null = null;
/** El monitor de la configuración que el usuario eligió para uno que no se reconoció. */
const bindChoice: Record<number, string> = {};
let copied = false;

const ICON_MONITOR = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><rect x="2" y="4" width="20" height="13" rx="2"/><path d="M8 21h8M12 17v4"/></svg>`;

function addmonDialog() {
  return $<HTMLDialogElement>("#addmon");
}

function openAddmon() {
  if (snap.kind !== "connected") return;
  if (calibration?.outcome) calibration = null;
  addmonError = null;
  copied = false;
  renderAddmon();
  if (!addmonDialog().open) addmonDialog().showModal();
  if (!calibration) void refreshDisplays();
}

async function refreshDisplays() {
  if (!bridge) return;
  displays = null;
  displaysError = null;
  renderAddmon();
  const r = await bridge.act({ kind: "list_displays" }).catch((e: unknown) => ({ kind: "unavailable" as const, reason: String(e) }));
  if (r.kind === "displays") displays = r.displays;
  else displaysError = r.kind === "rejected" || r.kind === "unavailable" ? r.reason : "el agente no devolvió la lista";
  renderAddmon();
}

/** Un pedido desde el diálogo: el motivo de un rechazo se muestra dentro de él. */
async function addmonAct(action: Action) {
  addmonError = await act(action, true);
  renderAddmon();
}

function copyProfile(toml: string) {
  const done = () => {
    copied = true;
    renderAddmon();
  };
  if (navigator.clipboard) navigator.clipboard.writeText(toml).then(done, () => (addmonError = "No se pudo copiar: selecciona el texto del perfil."));
  else addmonError = "No se pudo copiar: selecciona el texto del perfil.";
  renderAddmon();
}

function exportBlock(toml: string): Node {
  const box = el("details", "aexport");
  box.append(el("summary", undefined, "Ver el perfil (TOML)"), el("pre", undefined, toml));
  return box;
}

function renderAddmon() {
  const d = addmonDialog();
  if (!d.open && !calibration) return;
  const desk = snap.kind === "connected" ? snap.desk : null;
  const body: Node[] = [];
  const foot: Node[] = [];
  const title = (text: string) => {
    const h = el("h2", undefined, text);
    h.id = "addmon-title";
    body.push(h);
  };

  if (calibration) {
    const run = calibration;
    const label = labelOf(desk, run.monitor, run.name);
    const monitor = run.monitor ? desk?.monitors.find((m) => m.id === run.monitor) : undefined;
    title(headline(run, label));
    body.push(el("p", "ahint", hint(run)));
    const lanes = el("div", "alanes");
    for (const input of INPUTS) {
      const l = run.lanes[input];
      const row = el("div", `alane ${l.state}`);
      const who = monitor ? (Object.entries(monitor.wiring).find(([, i]) => i === input)?.[0] as Machine | undefined) : undefined;
      if (who) row.dataset.m = who;
      const name = el("span", "aport");
      name.append(el("b", undefined, SHORT[input]), el("small", undefined, who ? MACHINE_NAME[who] : PORT_NAME[input]));
      row.append(el("i"), name, el("span", "astate", laneText(l)));
      if (input === run.start) row.append(el("span", "ahome", "partida"));
      lanes.append(row);
    }
    body.push(lanes);
    if (question && !run.outcome) {
      const q = question;
      const text = questionText(q);
      const box = el("div", "aquestion");
      box.append(el("p", undefined, text.text));
      const row = el("div", "abtns");
      for (const c of text.choices) row.append(kbtn(c.label, () => void act({ kind: "answer_probe", id: q.id, answer: c.answer }), c.kind));
      box.append(row);
      body.push(box);
    }
    const out = run.outcome;
    if (out?.kind === "saved") {
      if (out.file) {
        const where = el("p", "afile", "Perfil guardado en ");
        where.append(el("code", undefined, out.file));
        body.push(where);
      }
      for (const note of out.notes) body.push(el("p", "pwarn", note));
      if (out.toml) body.push(exportBlock(out.toml));
    }
    if (addmonError) body.push(el("p", "pwarn", addmonError));
    if (!out) {
      foot.push(kbtn("Seguir en segundo plano", () => addmonDialog().close()), el("span", "spacer"), kbtn("Cancelar", () => void addmonAct({ kind: "cancel_calibration" }), "danger"));
    } else {
      const back = kbtn("Volver a la lista", () => {
        calibration = null;
        copied = false;
        void refreshDisplays();
      });
      foot.push(back, el("span", "spacer"));
      if (out.kind === "saved" && out.toml) {
        const toml = out.toml;
        foot.push(kbtn(copied ? "Copiado" : "Copiar perfil", () => copyProfile(toml)));
      }
      foot.push(kbtn("Listo", () => addmonDialog().close(), "primary"));
    }
  } else {
    title("Agregar monitor");
    body.push(el("p", "ahint", "Los monitores que ve la PC por DDC/CI. Los que no tienen perfil se calibran: Tandem prueba códigos y siempre vuelve a la entrada de partida."));
    if (displaysError) body.push(el("p", "pwarn", capitalize(displaysError)));
    else if (!displays) body.push(el("p", "ahint", "Buscando monitores…"));
    else if (!displays.length) body.push(el("p", "ahint", "La PC no ve ningún monitor con DDC/CI."));
    const free = unbound(desk, displays ?? []);
    const list = el("div", "alist");
    for (const dv of displays ?? []) {
      const row = el("div", "arow");
      const icon = el("span", "aicon");
      icon.innerHTML = ICON_MONITOR;
      const info = el("div", "ainfo");
      info.append(el("b", undefined, labelOf(desk, dv.monitor, dv.name)), el("small", undefined, [dv.name, dv.pnp_id].filter(Boolean).join(" · ")));
      const b = badge(dv.profile);
      const tag = el("span", `abadge ${b.kind}`, b.text);
      if (b.codes) tag.title = b.codes;
      const codes = el("span", "acodes", b.codes ?? "");
      const btns = el("div", "abtns");
      if (!dv.monitor && free.length) {
        const pick = el("select", "apick");
        pick.setAttribute("aria-label", `¿Cuál de la configuración es ${dv.name}?`);
        pick.append(new Option("¿Cuál es?", ""));
        for (const m of free) pick.append(new Option(m.label, m.id, false, bindChoice[dv.index] === m.id));
        pick.addEventListener("change", () => {
          if (pick.value) bindChoice[dv.index] = pick.value;
          else delete bindChoice[dv.index];
        });
        btns.append(pick);
      }
      const monitor = () => dv.monitor ?? bindChoice[dv.index] ?? null;
      for (const a of rowActions(dv)) {
        if (a === "calibrate" || a === "recalibrate") {
          btns.append(kbtn(a === "calibrate" ? "Calibrar" : "Calibrar de nuevo", () => void addmonAct({ kind: "start_calibration", display: dv.index, monitor: monitor() }), a === "calibrate" ? "primary" : ""));
        } else if (a === "use") {
          btns.append(kbtn("Usar perfil", () => void addmonAct({ kind: "use_profile", display: dv.index, monitor: monitor() }).then(refreshDisplays), "primary"));
        } else if (dv.profile.kind === "known") {
          const toml = dv.profile.toml;
          btns.append(kbtn("Exportar", () => copyProfile(toml)));
        }
      }
      row.append(icon, info, tag, codes, btns);
      list.append(row);
    }
    body.push(list);
    if (copied) body.push(el("p", "pok", "Perfil copiado: pégalo en un archivo <PNP>.toml para compartirlo."));
    if (addmonError) body.push(el("p", "pwarn", capitalize(addmonError)));
    foot.push(kbtn("Cerrar", () => addmonDialog().close()), el("span", "spacer"), kbtn("Buscar de nuevo", () => void refreshDisplays()));
  }
  $("#addmon-body").replaceChildren(...body);
  $("#addmon-foot").replaceChildren(...foot);
}

function wireAddmon() {
  $("#monitors-btn").addEventListener("click", openAddmon);
  // Cerrar no corta la calibración: sus avisos siguen en los monitores. Un resultado
  // ya visto se descarta.
  addmonDialog().addEventListener("close", () => {
    if (calibration?.outcome) calibration = null;
    addmonError = null;
  });
}

// ---------------------------------------------------------------------------------------
// Controles

function chooseMode(key: ModeKey) {
  if (snap.kind !== "connected") return;
  void act({ kind: "set_mode", mode: modeRequest(key, snap.desk, snap.state) });
}

function wire() {
  for (const m of $$<HTMLButtonElement>(".mode")) {
    m.addEventListener("click", () => chooseMode(m.dataset.mode as ModeKey));
  }
  // Los atajos que el sistema no dejó registrar como globales los atiende la ventana.
  addEventListener("keydown", (e) => {
    if (!keys || $<HTMLDialogElement>("#keys").open || $<HTMLDialogElement>("#setup").open || pairDialog().open || addmonDialog().open) return;
    const a = localAction(e, keys);
    if (!a) return;
    e.preventDefault();
    const cmd = windowCommand(a);
    if (cmd.kind === "mode") chooseMode(cmd.mode);
    else void bridge?.hideMain();
  });
  wireKeys();
  wireSetup();
  wirePair();
  wireAddmon();

  for (const side of $$(".side")) {
    const to = side.dataset.owner as Machine;
    side.addEventListener("dragover", (e) => {
      e.preventDefault();
      side.classList.add("drop");
    });
    side.addEventListener("dragleave", (e) => {
      if (!side.contains(e.relatedTarget as Node | null)) side.classList.remove("drop");
    });
    side.addEventListener("drop", (e) => {
      e.preventDefault();
      side.classList.remove("drop");
      const id = e.dataTransfer?.getData("text/plain");
      if (!id || snap.kind !== "connected") return;
      if ((snap.state.owner[id] ?? "mac") !== to) void act({ kind: "move_monitor", monitor: id, to });
    });
  }

  for (const b of $$<HTMLButtonElement>("#builtin-seg button")) {
    if (b.dataset.v === "gather") continue;
    b.addEventListener("click", () => {
      if (b.disabled || b.classList.contains("sel")) return;
      void act({ kind: "set_builtin", display: b.dataset.v as BuiltinDisplay });
    });
  }

  $("#power").addEventListener("click", () => {
    if (snap.kind !== "connected" || isRunning(progress)) return;
    void act({ kind: "set_pc_power", on: !snap.state.pc_on });
  });
}

async function pickBridge(): Promise<Bridge> {
  // `import.meta.env.DEV` es constante en el build: la rama y el agente simulado desaparecen.
  if (import.meta.env.DEV && new URLSearchParams(location.search).has("demo")) {
    const { demoBridge } = await import("./demo");
    return demoBridge(location.search);
  }
  return tauriBridge();
}

async function boot() {
  wire();
  render([]);
  try {
    bridge = await pickBridge();
    let pushed = false;
    await bridge.onSnapshot((s) => {
      pushed = true;
      setSnapshot(s);
    });
    await bridge.onNotice(onNotice);
    await bridge.onMessage((text) => showMessage(text));
    await bridge.onShortcuts((view) => {
      keys = view;
      renderModeKeys();
    });
    keys = await bridge.shortcuts().catch(() => null);
    $("#keys-btn").hidden = keys === null;
    renderModeKeys();
    setupView = await bridge.setup().catch(() => null);
    $("#setup-btn").hidden = !setupView?.available;
    if (setupView?.show) openSetup();
    const first = await bridge.snapshot();
    // Si ya llegó uno empujado, es más nuevo que esta respuesta.
    if (!pushed) setSnapshot(first);
    // Modo demo: `/?demo&addmon` abre «Agregar monitor».
    if (import.meta.env.DEV && new URLSearchParams(location.search).has("addmon")) openAddmon();
  } catch (e) {
    setSnapshot(bridgeFailed(e));
  }
}

void boot();
