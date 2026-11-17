// La mini escena que abre el ícono de la barra de menú (macOS) o de la bandeja (Windows):
// las dos máquinas, el enlace y el avance del encendido o apagado en curso. Los modos están
// en pausa hasta el firmware propio (DESIGN §5): se ven deshabilitados.

import "./styles.css";
import "./tray.css";

import { bridgeFailed, tauriBridge, type Bridge } from "./bridge";
import type { Notice, Snapshot } from "./model";
import { MACHINE_NAME, advance, learnNames, isRunning, machineState, peerPill, peerPower, sides, stepLabel, type Progress } from "./scene";

const MESSAGE_MS = 5000;

const $ = <T extends Element = HTMLElement>(sel: string, root: ParentNode = document) => root.querySelector(sel) as T;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

let bridge: Bridge | null = null;
let snap: Snapshot = { kind: "connecting" };
let progress: Progress | null = null;
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

function onNotice(n: Notice) {
  if (n.kind === "power") {
    say(n.reason);
    return;
  }
  if (n.kind !== "txn") return;
  progress = advance(progress, n.event);
  const p = progress;
  if (p && p.outcome.kind === "failed") say(`No se pudo: ${p.outcome.error}`);
  if (p && !isRunning(p)) progress = null;
  render();
}

function render() {
  learnNames(snap);
  const connected = snap.kind === "connected";
  $("#agent-text").textContent =
    snap.kind === "connected" ? `tandemd · ${MACHINE_NAME[snap.agent]}` : snap.kind === "connecting" ? "Conectando…" : "Sin agente";
  $("#agent-dot").classList.toggle("warn", !connected);
  $("#mini").hidden = !connected;
  $("#stage").hidden = connected;
  renderStage();
  renderMini();
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
  const { state, agent, desk, peer } = snap;
  const power = peerPower(state, agent, progress);
  const both = sides(agent, desk);
  for (const [which, who] of [
    ["here", both.left],
    ["peer", both.right],
  ] as const) {
    const side = $(`.mside[data-side="${which}"]`);
    side.className = `mside ${who}`;
    side.dataset.owner = who;
    side.classList.toggle("asleep", who !== agent && power !== "on");
    $("[data-part=name]", side).textContent = MACHINE_NAME[who];
    $("[data-part=state]", side).textContent = who === agent ? "" : machineState(state, agent, who, progress);
    $(".mdesk", side).replaceChildren();
  }
  const pill = peerPill(peer);
  const link = $(".mlink");
  link.title = pill.title;
  link.classList.toggle("down", !pill.ok);
}

function renderFoot() {
  const msg = $("#msg");
  msg.className = "pmsg";
  if (progress) {
    const p = progress;
    const i = Math.max(0, p.status.findIndex((s) => s !== "done"));
    msg.classList.add("run");
    msg.textContent = p.steps[i] ? stepLabel(p.steps[i]) : "Cambiando";
  } else if (message) {
    msg.classList.add("warn");
    msg.textContent = message;
  } else {
    msg.textContent = "";
  }
}

function wire() {
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
    const first = await bridge.snapshot();
    if (!pushed) snap = first;
    render();
  } catch (e) {
    snap = bridgeFailed(e);
    render();
  }
}

void boot();
