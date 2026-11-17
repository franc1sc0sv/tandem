import "./styles.css";

import { bridgeFailed, tauriBridge, type Bridge } from "./bridge";
import type { Action, Bindings, Notice, PairingView, Permission, SetupView, ShortcutAction, ShortcutsView, Snapshot } from "./model";
import { isActive, needsDismiss, pairLine, pairScreen, shouldOpen } from "./pairing";
import {
  MACHINE_NAME,
  learnNames,
  machineInfo,
  MODES_SOON,
  MONITORS_SOON,
  SOON,
  advance,
  capsOf,
  declaredScreens,
  isRunning,
  machineState,
  other,
  peerPill,
  peerPower,
  powerButton,
  roleChip,
  sides,
  stepLabel,
  type Progress,
  type RoleChip,
} from "./scene";
import { ACTION_LABEL, ACTIONS, DEFAULT_BINDINGS, duplicateOf, formatAccel, isMacPlatform, localAction, readCombo, windowCommand } from "./shortcuts";
import { initOnboarding, type Onboard } from "./onboarding-ui";
import { MISSING_HELP, grantTo, permissionState } from "./permissions";
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
/** El asistente de configuración inicial (DESIGN §5). */
let onboard: Onboard | null = null;
let snap: Snapshot = { kind: "connecting" };
let progress: Progress | null = null;
let message: { text: string; sticky: boolean } | null = null;
let messageTimer: number | undefined;
let keys: ShortcutsView | null = null;
let lastPairing: PairingView | null = null;
const MAC = isMacPlatform();

// ---------------------------------------------------------------------------------------
// Estado

/** Lo que la guía muestra del agente: re-dibujarla solo si cambia, para no quitarle el foco al teclado. */
function permissionsKey(s: Snapshot): string {
  if (s.kind !== "connected") return "";
  const i = s.caps.here.input;
  return `${i.missing.join(",")}|${i.process.pid}|${i.process.started}`;
}
let lastPermissionsKey = "";

function setSnapshot(next: Snapshot) {
  const pairingBefore = lastPairing;
  lastPairing = next.kind === "connected" ? next.pairing : null;
  snap = next;
  render();
  syncPair(pairingBefore);
  onboard?.onSnapshot(next);
  // La guía de permisos muestra lo que el agente sabe: se pone al día con cada estado (T-090).
  if (wizard && $<HTMLDialogElement>("#setup").open && permissionsKey(next) !== lastPermissionsKey) renderSetup();
  lastPermissionsKey = permissionsKey(next);
  // Instalación limpia: el asistente se abre solo.
  if (next.kind === "connected" && !$<HTMLDialogElement>("#setup").open) onboard?.maybeOpen(next);
}

function onNotice(n: Notice) {
  onboard?.onNotice(n);
  if (n.kind === "key_probe") return;
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
    if (p && p.outcome.kind === "failed") {
      showMessage(`No se pudo: ${p.outcome.error}.`);
      progress = null;
    }
    renderControls();
    renderToast();
  } else if (n.kind === "power") {
    showMessage(n.reason, true);
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

/**
 * Un monitor deshabilitado: el control de monitores está en pausa hasta el firmware propio
 * (DESIGN §8). Se dibuja para que el puesto se reconozca, sin nada que tocar.
 */
function monitorSoon(label: string, wide: boolean) {
  const node = el("div", `monitor soon ${wide ? "wide" : "std"}`);
  node.innerHTML = `<div class="screen"><div class="wall"></div></div><div class="stand"></div><div class="base"></div>`;
  const tag = el("div", "mlabel");
  tag.append(el("b", undefined, label));
  node.append(tag);
  node.title = SOON;
  node.setAttribute("aria-label", `${label}: ${SOON}`);
  node.setAttribute("aria-disabled", "true");
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

const SIDES = ["left", "right"] as const;

function sideEl(which: (typeof SIDES)[number]) {
  return $(`.side[data-side="${which}"]`);
}

/**
 * Las dos tarjetas en el orden del acomodo (DESIGN §7): la Mac a la izquierda salvo que el
 * puesto diga otra cosa. Cada una con su equipo, sus pantallas declaradas y, en la que no
 * es esta, el botón de encendido.
 */
function renderSides() {
  if (snap.kind !== "connected") return;
  const { agent, caps, desk } = snap;
  const both = sides(agent, desk);
  for (const which of SIDES) {
    const who = both[which];
    const side = sideEl(which);
    side.className = `side ${who}`;
    side.dataset.owner = who;
    const info = machineInfo(snap, who);
    if (info) side.style.setProperty("--accent", info.color);
    else side.style.removeProperty("--accent");
    const name = $("[data-part=name]", side);
    name.textContent = MACHINE_NAME[who];
    const announced = capsOf(caps, who);
    name.title = announced ? `${announced.name} · ${announced.os}` : who !== agent ? "todavía no se la vio" : "";
    side.setAttribute("aria-label", who === agent ? `${MACHINE_NAME[who]} (esta máquina)` : MACHINE_NAME[who]);
    const slot = $(".hw-slot", side);
    // El equipo se dibuja por su sistema, no por su lado (sin saberlo, una torre).
    const hw = $(info?.os === "macos" ? "#laptop" : "#tower", $("#parts")).cloneNode(true) as Element;
    hw.removeAttribute("id");
    slot.replaceChildren(hw);
    const declared = declaredScreens(desk, who);
    $("[data-part=screens]", side).textContent = declared.length
      ? `${declared.length} ${declared.length === 1 ? "pantalla declarada" : "pantallas declaradas"}`
      : "pantallas que lee el sistema";
    const powerSlot = $(".power-slot", side);
    const pw = $("#power");
    if (who !== agent && pw.parentElement !== powerSlot) powerSlot.replaceChildren(pw);
  }
}

/** Los monitores del puesto, deshabilitados, entre las dos máquinas. */
function renderMonitors() {
  const box = $("#monitors-soon");
  if (box.childElementCount) return;
  const row = el("div", "soon-row");
  row.append(...MONITORS_SOON.map((m) => monitorSoon(m.label, m.wide)));
  box.replaceChildren(row, el("p", "soon-note", SOON));
}

/** Lo que cambia también con el avance de una transacción. */
function renderControls() {
  if (snap.kind !== "connected") return;
  const { state, agent, caps, desk } = snap;
  const running = isRunning(progress);
  const power = peerPower(state, agent, progress);
  renderSides();
  const both = sides(agent, desk);

  for (const which of SIDES) {
    const who = both[which];
    const side = sideEl(which);
    // Esta máquina nunca está apagada para sí misma (T-084).
    $("[data-part=state]", side).textContent = machineState(state, agent, who, progress);
    const role = $("[data-part=role]", side);
    const chip = roleChip(snap.input, agent, who, power === "on");
    if (chip) {
      role.className = `role ${chip.kind}`;
      role.innerHTML = `${ROLE_ICON[chip.icon]} ${chip.text}`;
      role.title = chip.title;
      role.hidden = false;
    } else {
      role.hidden = true;
    }
    side.classList.toggle("asleep", who !== agent && power !== "on");
  }

  // Solo si alguna máquina puede: despertar a la otra, o que ella se apague limpio.
  const button = powerButton(caps, state, agent, progress);
  const pw = $<HTMLButtonElement>("#power");
  pw.hidden = !button.visible;
  pw.className = `power ${button.power === "off" ? "" : button.power}`.trim();
  pw.setAttribute("aria-label", button.label);
  pw.title = button.label;
  pw.setAttribute("aria-disabled", String(running || button.on === null));

  const linkPill = $("#link-pill");
  const pill = peerPill(snap.peer);
  linkPill.hidden = false;
  $("#link-text").textContent = pill.text;
  $("#link-dot").classList.toggle("warn", !pill.ok);
  $("#link-act").hidden = snap.peer.kind === "paired";
  linkPill.title = snap.peer.kind === "paired" ? `${pill.title}. Clic: ver el par` : `${pill.title}. Clic: emparejar`;
  // El enlace activo, encendido en la escena.
  const link = snap.peer.kind === "paired" ? snap.peer.link : null;
  for (const l of $$("#links .link")) l.classList.toggle("down", l.dataset.k !== link);
}

function renderToast() {
  const t = $("#toast");
  const parts: Node[] = [];
  if (progress) {
    const p = progress;
    p.steps.forEach((step, i) => {
      if (i) parts.push(el("span", "sep"));
      const s = el("span", `step ${p.status[i]}`);
      s.append(el("i"), stepLabel(step));
      parts.push(s);
    });
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

/** Las tarjetas de modo, deshabilitadas con su leyenda (DESIGN §5). */
function renderModes() {
  const box = $("#modes");
  if (box.childElementCount) return;
  box.append(
    ...MODES_SOON.map((label) => {
      const b = el("button", "mode soon");
      b.type = "button";
      b.disabled = true;
      b.title = SOON;
      b.setAttribute("aria-label", `${label}: ${SOON}`);
      b.append(el("div", "t", label), el("span", "k", "próximamente"));
      return b;
    }),
  );
}

function render() {
  learnNames(snap);
  renderAgentPill();
  const connected = snap.kind === "connected";
  $("#scene").hidden = !connected;
  $("#bottom").hidden = !connected;
  $("#stage").hidden = connected;
  $("#onboard-btn").hidden = !connected;
  if (!connected) $("#link-pill").hidden = true;
  renderUnavailable();
  renderMonitors();
  renderModes();
  renderControls();
  renderToast();
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
    body.push(el("p", undefined, "No hay que escribir ninguna configuración: tandemd detecta los monitores y la crea solo. La ves, y cambias el cableado, en «Puesto»."));
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
    // Lo que el agente sabe: si falta y a qué binario concederlo (T-090).
    const here = snap.kind === "connected" ? snap.caps.here : null;
    const state = permissionState(here, p);
    if (state !== "unknown") {
      const line = el("div", "sagent");
      line.append(el("span", `dot${state === "granted" ? "" : " warn"}`), state === "granted" ? "Concedido: tandemd ya puede usarlo" : "Falta: tandemd todavía no lo tiene");
      body.push(line);
    }
    const binary = grantTo(here);
    if (binary) body.push(el("p", "how", binary));
    if (state === "missing") body.push(el("p", "how", MISSING_HELP));
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
    if (state !== "unknown") {
      const ask = el("button", "kbtn sopen", "Pedir permisos");
      ask.type = "button";
      ask.addEventListener("click", () => {
        setupError = null;
        void act({ kind: "request_permissions" });
      });
      const restart = el("button", "kbtn sopen", "Reiniciar agente");
      restart.type = "button";
      restart.addEventListener("click", () => {
        setupError = null;
        bridge?.restartAgent().catch((e: unknown) => {
          setupError = `No se pudo reiniciar el agente: ${e instanceof Error ? e.message : String(e)}`;
          renderSetup();
        });
      });
      body.push(ask, restart);
    }
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

function wire() {
  // Los atajos que el sistema no dejó registrar como globales los atiende la ventana.
  addEventListener("keydown", (e) => {
    if (!keys || $<HTMLDialogElement>("#keys").open || $<HTMLDialogElement>("#setup").open || pairDialog().open) return;
    const a: ShortcutAction | null = localAction(e, keys);
    if (!a) return;
    e.preventDefault();
    if (windowCommand(a).kind === "hide_main") void bridge?.hideMain();
  });
  wireKeys();
  wireSetup();
  wirePair();
  $("#onboard-btn").addEventListener("click", () => onboard?.open());
  // La guía de permisos va antes: al cerrarla, el asistente se abre si hace falta.
  $<HTMLDialogElement>("#setup").addEventListener("close", () => onboard?.maybeOpen(snap));

  $("#power").addEventListener("click", () => {
    if (snap.kind !== "connected" || isRunning(progress)) return;
    const button = powerButton(snap.caps, snap.state, snap.agent, progress);
    if (!button.visible || button.on === null) return;
    void act({ kind: "set_power", machine: other(snap.agent), on: button.on });
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
  render();
  try {
    bridge = await pickBridge();
    onboard = initOnboarding({
      snap: () => snap,
      request: (action) => bridge!.act(action).catch((e: unknown) => ({ kind: "unavailable" as const, reason: String(e) })),
      bridge: () => bridge,
      message: (text) => showMessage(text),
    });
    let pushed = false;
    await bridge.onSnapshot((s) => {
      pushed = true;
      setSnapshot(s);
    });
    await bridge.onNotice(onNotice);
    await bridge.onMessage((text) => showMessage(text));
    await bridge.onShortcuts((view) => {
      keys = view;
    });
    keys = await bridge.shortcuts().catch(() => null);
    $("#keys-btn").hidden = keys === null;
    setupView = await bridge.setup().catch(() => null);
    $("#setup-btn").hidden = !setupView?.available;
    if (setupView?.show) openSetup();
    const first = await bridge.snapshot();
    // Si ya llegó uno empujado, es más nuevo que esta respuesta.
    if (!pushed) setSnapshot(first);
  } catch (e) {
    setSnapshot(bridgeFailed(e));
  }
}

void boot();
