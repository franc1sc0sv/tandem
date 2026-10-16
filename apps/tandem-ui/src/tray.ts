// La mini escena que abre el ícono de la barra de menú (macOS) o de la bandeja (Windows):
// las dos máquinas con sus monitores, los tres modos y el avance del cambio en curso.

import "./styles.css";
import "./tray.css";

import { bridgeFailed, tauriBridge, type Bridge } from "./bridge";
import type { Action, Notice, ShortcutsView, Snapshot } from "./model";
import {
  MACHINE_NAME,
  advance,
  groupStatus,
  groupSteps,
  isRunning,
  modeOf,
  modeRequest,
  monitorsOf,
  machineState,
  other,
  peerPower,
  sides,
  type ModeKey,
  type Progress,
} from "./scene";
import { ACTION_OF_MODE, formatAccel, isMacPlatform } from "./shortcuts";

const MESSAGE_MS = 5000;
const MAC = isMacPlatform();

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
let keys: ShortcutsView | null = null;
let message: string | null = null;
let messageTimer: number | undefined;

function say(text: string) {
  message = text;
  window.clearTimeout(messageTimer);
  messageTimer = window.setTimeout(() => {
    message = null;
    renderFoot();
  }, MESSAGE_MS);
  renderFoot();
}

async function act(action: Action) {
  if (!bridge) return;
  const r = await bridge.act(action).catch((e: unknown) => ({ kind: "unavailable" as const, reason: String(e) }));
  if (r.kind === "rejected") say(r.reason);
  else if (r.kind === "unavailable") say(`Sin agente: ${r.reason}`);
}

function onNotice(n: Notice) {
  if (n.kind === "power") {
    say(n.reason);
    return;
  }
  // Las sondas de calibración tienen su ventana, y la pregunta trae al frente la principal.
  if (n.kind === "probe") return;
  if (n.kind !== "txn") {
    // El cambio manual de entrada se contesta en la ventana principal.
    if (n.event.type === "requested") say(`Cambia el ${n.event.label} a mano: abre Tandem`);
    return;
  }
  progress = advance(progress, n.event);
  const p = progress;
  if (p && p.outcome.kind === "rolled_back") say(`No se pudo: ${p.outcome.error}. Todo volvió como estaba.`);
  if (p && p.outcome.kind === "inconsistent") say(`No se pudo deshacer todo: ${p.outcome.error}`);
  if (p && !isRunning(p)) progress = null;
  render();
}

function render() {
  const connected = snap.kind === "connected";
  $("#agent-text").textContent =
    snap.kind === "connected" ? `tandemd · ${MACHINE_NAME[snap.agent]}` : snap.kind === "connecting" ? "Conectando…" : "Sin agente";
  $("#agent-dot").classList.toggle("warn", !connected);
  $("#mini").hidden = !connected;
  $("#stage").hidden = connected;
  renderStage();
  renderMini();
  renderModes();
  renderFoot();
}

function renderStage() {
  const stage = $("#stage");
  if (snap.kind !== "unavailable") {
    stage.replaceChildren(snap.kind === "connecting" ? el("p", "mreason", "Conectando con tandemd…") : "");
    return;
  }
  const retry = el("button", "popen", "Reintentar");
  retry.type = "button";
  retry.addEventListener("click", () => void bridge?.retry());
  stage.replaceChildren(el("p", "mreason", snap.reason), retry);
}

function renderMini() {
  if (snap.kind !== "connected") return;
  const { desk, state, agent, caps } = snap;
  const running = isRunning(progress);
  const power = peerPower(state, agent, progress);
  const both = sides(agent);
  for (const which of ["here", "peer"] as const) {
    const who = both[which];
    const side = $(`.mside[data-side="${which}"]`);
    side.className = `mside ${who}`;
    side.dataset.owner = who;
    side.classList.toggle("asleep", which === "peer" && power !== "on");
    $("[data-part=name]", side).textContent = MACHINE_NAME[who];
    $("[data-part=state]", side).textContent = which === "here" ? "" : machineState(state, agent, who, progress);
    const mine = monitorsOf(desk, state, who, caps);
    const deskEl = $(".mdesk", side);
    deskEl.dataset.owner = who;
    deskEl.replaceChildren(
      ...mine.map((m) => {
        const to = other(m.owner);
        const b = el("button", `mmon ${m.owner} ${m.wide ? "wide" : "std"}`);
        b.type = "button";
        b.append(el("span", "mscreen"), el("span", "mlab", m.label));
        b.disabled = running || !m.movable;
        b.title = m.movable ? `Pasar el ${m.label} a la ${MACHINE_NAME[to]}` : m.label;
        b.setAttribute("aria-label", `${m.label} en ${MACHINE_NAME[m.owner]}. Pasarlo a la ${MACHINE_NAME[to]}`);
        b.addEventListener("click", () => void act({ kind: "move_monitor", monitor: m.id, to }));
        return b;
      }),
    );
    if (!mine.length) deskEl.append(el("span", "mempty", which === "peer" && power === "off" ? "apagada" : "sin monitores"));
  }
}

function renderModes() {
  const connected = snap.kind === "connected";
  const mode = snap.kind === "connected" ? modeOf(snap.desk, snap.state) : null;
  const running = isRunning(progress);
  for (const b of $$<HTMLButtonElement>(".pmode")) {
    const key = b.dataset.mode as ModeKey;
    const sel = key === mode;
    b.classList.toggle("sel", sel);
    b.setAttribute("aria-pressed", String(sel));
    b.disabled = !connected || mode === null || running;
    $(".k", b).textContent = formatAccel(keys?.bindings[ACTION_OF_MODE[key]], MAC);
  }
}

function renderFoot() {
  const msg = $("#msg");
  msg.className = "pmsg";
  if (progress && snap.kind === "connected") {
    const p = progress;
    const groups = groupSteps(snap.desk, p.steps);
    const current = groups.find((g) => groupStatus(p, g) !== "done") ?? groups[groups.length - 1];
    const n = groups.indexOf(current) + 1;
    msg.classList.add("run");
    msg.textContent = p.outcome.kind === "rolling_back" ? `Deshaciendo: ${p.outcome.error}` : `${current?.label ?? "Cambiando"} · ${n}/${groups.length}`;
  } else if (message) {
    msg.classList.add("warn");
    msg.textContent = message;
  } else {
    msg.textContent = "";
  }
}

function wire() {
  for (const b of $$<HTMLButtonElement>(".pmode")) {
    b.addEventListener("click", () => {
      if (snap.kind !== "connected") return;
      void act({ kind: "set_mode", mode: modeRequest(b.dataset.mode as ModeKey, snap.desk, snap.state) });
    });
  }
  $("#open").addEventListener("click", () => void bridge?.showMain());
  addEventListener("keydown", (e) => {
    if (e.key === "Escape") void bridge?.hideTray();
  });
}

async function pickBridge(): Promise<Bridge> {
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
    let pushed = false;
    await bridge.onSnapshot((s) => {
      pushed = true;
      snap = s;
      render();
    });
    await bridge.onNotice(onNotice);
    await bridge.onMessage(say);
    // La ventana se esconde pero no se destruye: los atajos cambiados llegan por aviso.
    await bridge.onShortcuts((view) => {
      keys = view;
      renderModes();
    });
    keys = await bridge.shortcuts().catch(() => null);
    const first = await bridge.snapshot();
    if (!pushed) snap = first;
    render();
  } catch (e) {
    snap = bridgeFailed(e);
    render();
  }
}

void boot();
