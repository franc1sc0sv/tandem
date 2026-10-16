import "./styles.css";

import { bridgeFailed, tauriBridge, type Bridge } from "./bridge";
import type { Action, BuiltinDisplay, Machine, ManualView, Notice, Snapshot } from "./model";
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
  shortcutMode,
  type ModeKey,
  type MonitorView,
  type Progress,
} from "./scene";

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
let message: { text: string; sticky: boolean } | null = null;
let messageTimer: number | undefined;
let lastOwner: Record<string, Machine> | null = null;

// ---------------------------------------------------------------------------------------
// Estado

function setSnapshot(next: Snapshot) {
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

async function act(action: Action) {
  if (!bridge) return;
  const r = await bridge.act(action).catch((e: unknown) => ({ kind: "unavailable" as const, reason: String(e) }));
  if (r.kind === "rejected") showMessage(r.reason);
  else if (r.kind === "unavailable") showMessage(`Sin agente: ${r.reason}`);
}

// ---------------------------------------------------------------------------------------
// Dibujo

const ICON_KB = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/></svg>`;
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
    // El agente todavía no informa dónde está el hub: chip neutro en esta máquina.
    const role = $(`#role-${who}`);
    if (who === agent) {
      role.className = "role unknown";
      role.innerHTML = `${ICON_KB} sin datos`;
      role.title = "Teclado y mouse: el agente todavía no informa quién es el servidor";
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

  // Sin estado del enlace en la IPC: los tres se ven sin datos y los servicios, bloqueados.
  const linkPill = $("#link-pill");
  linkPill.hidden = false;
  $("#link-text").textContent = "enlace sin datos";
  linkPill.title = "El agente todavía no informa el estado de los enlaces";
}

function renderToast() {
  const t = $("#toast");
  const parts: Node[] = [];
  if (manual) {
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
  if (!connected) $("#link-pill").hidden = true;
  renderUnavailable();
  renderDesks(moved);
  renderControls();
  renderToast();
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
  addEventListener("keydown", (e) => {
    const key = shortcutMode(e);
    if (!key) return;
    e.preventDefault();
    chooseMode(key);
  });

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
    const first = await bridge.snapshot();
    // Si ya llegó uno empujado, es más nuevo que esta respuesta.
    if (!pushed) setSnapshot(first);
  } catch (e) {
    setSnapshot(bridgeFailed(e));
  }
}

void boot();
