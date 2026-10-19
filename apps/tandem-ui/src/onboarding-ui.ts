// El asistente de configuración inicial en pantalla (T-087): dibuja el `StepView` del paso
// actual y hace de puente entre sus botones y el agente. La lógica de qué se muestra y cuándo
// se avanza está en `onboarding.ts`, donde se prueba sin DOM.

import type { Bridge } from "./bridge";
import type { Action, ActionResult, Machine, Notice, Permission, Snapshot } from "./model";
import {
  STEPS,
  addAction,
  advanceTo,
  answerChoice,
  begin,
  canAdvance,
  countsAsKeyProbe,
  goBack,
  indexOf,
  shouldOpen,
  verifyStep,
  viewOf,
  AskDeadline,
  abandonOnClose,
  homeOf,
  keepInFlight,
  moveEnded,
  type Abandoned,
  type Btn,
  type Check,
  type Onboarding,
  type Row,
  type VerifyEntry,
  type StepView,
  type VerifyEffect,
  type VerifyEvent,
} from "./onboarding";
import { modeRequest, type ModeKey } from "./scene";

/** Cada cuánto se vuelve a buscar mientras el paso de monitores está a la vista. */
const SCAN_EVERY_MS = 2000;

export interface Host {
  snap(): Snapshot;
  /** Pide algo al agente y devuelve su respuesta tal cual. */
  request(action: Action): Promise<ActionResult>;
  bridge(): Bridge | null;
  message(text: string): void;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

const reasonOf = (r: ActionResult): string | null =>
  r.kind === "rejected" ? r.reason : r.kind === "unavailable" ? `sin agente: ${r.reason}` : null;

const MARK: Record<Check["state"], string> = { ok: "✓", wait: "…", fail: "!", idle: "·" };

export interface Onboard {
  open(): void;
  isOpen(): boolean;
  /** Abre solo si el puesto está vacío (instalación limpia o restablecido) y no se descartó. */
  maybeOpen(snap: Snapshot): void;
  onSnapshot(snap: Snapshot): void;
  onNotice(n: Notice): void;
}

export function initOnboarding(host: Host): Onboard {
  const dialog = document.querySelector<HTMLDialogElement>("#onboard")!;
  const body = document.querySelector<HTMLElement>("#onboard-body")!;
  const dots = document.querySelector<HTMLElement>("#onboard-dots")!;
  const foot = document.querySelector<HTMLElement>("#onboard-foot")!;

  let o: Onboarding = begin();
  /** El cambio de modo que el asistente espera, para saber cuándo terminó. */
  let waiting: { txn: number; then: "move"; monitor: string } | { txn: number; then: "apply" } | null = null;
  let scanTimer: number | undefined;
  /** Se cerró a mano: no se vuelve a abrir solo en esta sesión. */
  let dismissed = false;
  let opened = false;

  const set = (next: Onboarding) => {
    o = next;
    render();
  };

  // ---- Efectos

  const disarmKeys = () => {
    if (o.keyboard.armed) void host.request({ kind: "key_probe", on: false });
  };

  async function scanNow() {
    if (o.monitors.scanning) return;
    set({ ...o, monitors: { ...o.monitors, scanning: true } });
    const r = await host.request({ kind: "scan_monitors" }).catch((e: unknown) => ({ kind: "unavailable" as const, reason: String(e) }));
    const why = reasonOf(r);
    set({
      ...o,
      monitors: { ...o.monitors, scanning: false, scan: r.kind === "scan" ? r.scan : o.monitors.scan, error: why },
    });
  }

  function syncPolling() {
    const want = dialog.open && o.step === "monitors" && !o.monitors.adding;
    if (want && scanTimer === undefined) {
      scanTimer = window.setInterval(() => void scanNow(), SCAN_EVERY_MS);
      void scanNow();
    } else if (!want && scanTimer !== undefined) {
      window.clearInterval(scanTimer);
      scanTimer = undefined;
    }
  }

  async function runEffects(effects: VerifyEffect[]) {
    for (const e of effects) {
      if (e.kind === "mark_manual") {
        await host.request({ kind: "mark_manual", monitor: e.monitor, discard_codes: e.discard });
        continue;
      }
      const r = await host.request({ kind: "probe_input", monitor: e.monitor, to: e.to, back: e.back });
      if (r.kind === "accepted") {
        waiting = { txn: r.txn, then: "move", monitor: e.monitor };
      } else if (r.kind === "done") {
        // La vuelta pedida justo cuando corre la automática: ya va en camino (no es un fallo).
        verify(e.monitor, { type: "moved", ok: true });
      } else {
        settle(e.monitor, "rejected");
        verify(e.monitor, { type: "moved", ok: false, reason: reasonOf(r) ?? "no se pudo pasar el monitor" });
      }
    }
  }

  /** Los monitores cuya primera ida se está completando cuando se cierra el asistente. */
  let abandoned: Abandoned = new Map();
  /** Cierra el rastro de un movimiento: borra la marca y, si tocaba, manda el monitor de vuelta. */
  /** La vuelta automática tras cerrar: si no se acepta o se deshace, se avisa (el monitor quedó en la otra máquina). */
  async function sendHome(monitor: string, to: Machine) {
    const r = await host.request({ kind: "probe_input", monitor, to, back: to });
    if (r.kind === "accepted") returning.set(r.txn, monitor);
    else host.message(`No se pudo devolver el monitor a la ${to === "pc" ? "PC" : "Mac"}: ${reasonOf(r) ?? "sin respuesta"}. Revísalo.`);
  }
  /** Vueltas automáticas en curso: txn → monitor. */
  const returning = new Map<number, string>();
  const settle = (monitor: string, end: Parameters<typeof moveEnded>[2]) => {
    const r = moveEnded(abandoned, monitor, end);
    abandoned = r.abandoned;
    if (r.returnTo) void sendHome(monitor, r.returnTo);
    return r.returnTo !== null;
  };
  const deadline = new AskDeadline({ set: (fn, ms) => window.setTimeout(fn, ms), clear: (h) => window.clearTimeout(h as number) }, (monitor) => {
    // Sin respuesta cuenta como «no»: el monitor vuelve solo a donde estaba.
    verify(monitor, { type: "timeout" });
  });

  function verify(monitor: string, e: VerifyEvent) {
    const s = host.snap();
    // Sin saber dónde está el monitor no hay a dónde devolverlo: no se manda nada.
    const home = s.kind === "connected" ? homeOf(o, s.state, monitor) : null;
    const entry = o.verify[monitor] ?? (home ? { home, phase: { kind: "idle" as const } } : null);
    if (!entry) return;
    const r = verifyStep(monitor, entry, e);
    set({ ...o, verify: { ...o.verify, [monitor]: r.next } });
    deadline.track(monitor, r.next.phase.kind);
    void runEffects(r.effects);
  }

  async function calibrate(monitor: string) {
    const listed = await host.request({ kind: "list_displays" });
    if (listed.kind !== "displays") {
      host.message(reasonOf(listed) ?? "no se pudo leer el bus DDC");
      return;
    }
    const s = host.snap();
    const cfg = s.kind === "connected" ? s.desk.monitors.find((m) => m.id === monitor) : undefined;
    const models = [cfg?.edid_model, ...(cfg?.edid_aliases ?? [])].filter((x): x is string => !!x).map((x) => x.toUpperCase());
    const d = listed.displays.find((x) => x.monitor === monitor) ?? listed.displays.find((x) => x.pnp_id && models.includes(x.pnp_id.toUpperCase()));
    if (!d) {
      host.message("la PC no ve ese monitor por DDC: se probará en modo manual asistido");
      return;
    }
    const r = await host.request({ kind: "start_calibration", display: d.index, monitor });
    const why = reasonOf(r);
    if (why) host.message(why);
  }

  async function press(b: Btn) {
    const id = b.id;
    const [kind, a, c] = id.split(":");
    switch (kind) {
      case "perm": {
        // «Pedir permisos» y «Reiniciar agente» (T-090); el resto abre el panel de Ajustes.
        if (a === "request") {
          const why = reasonOf(await host.request({ kind: "request_permissions" }));
          return set({ ...o, error: why });
        }
        if (a === "restart") {
          try {
            await host.bridge()?.restartAgent();
            host.message("tandemd se reinicia: en unos segundos se vuelven a comprobar los permisos");
            return set({ ...o, error: null });
          } catch (e) {
            return set({ ...o, error: `No se pudo reiniciar el agente: ${e instanceof Error ? e.message : String(e)}` });
          }
        }
        await host.bridge()?.openSettings(a as Permission);
        return;
      }
      case "pair": {
        if (a === "skip") return set({ ...o, pair: { ...o.pair, skipped: true } });
        const action = a === "start" ? "start_pairing" : a === "confirm" ? "confirm_pairing" : "cancel_pairing";
        const why = reasonOf(await host.request({ kind: action }));
        return set({ ...o, error: why });
      }
      case "ping": {
        const r = await host.request({ kind: "ping_peer" });
        return set({
          ...o,
          pair: r.kind === "pong" ? { ...o.pair, ping: { link: r.link, millis: r.millis }, pingError: null } : { ...o.pair, ping: null, pingError: reasonOf(r) ?? "sin respuesta" },
        });
      }
      case "key":
        return pressKey(a!, c);
      case "scan":
        return void scanNow();
      case "link":
      case "wire": {
        const next = answerChoice(o, id);
        if (next) set(next);
        return;
      }
      case "add": {
        if (a !== undefined) {
          // «add:<clave>» alterna la fila; sin clave es el botón de agregar.
          const next = answerChoice(o, id);
          if (next) set(next);
          return;
        }
        const s = host.snap();
        if (s.kind !== "connected") return;
        set({ ...o, monitors: { ...o.monitors, adding: true } });
        const why = reasonOf(await host.request(addAction(o, s.agent)));
        set({ ...o, error: why, monitors: { ...o.monitors, adding: false, added: why === null } });
        if (!why) void scanNow();
        return;
      }
      case "monitors":
        return set({ ...o, monitors: { ...o.monitors, skipped: true } });
      case "home":
        // Lo que el usuario ve ahora: de ahí sale y ahí vuelve la prueba.
        return set({ ...o, verify: { ...o.verify, [a!]: { home: c as Machine, phase: { kind: "idle" } } } });
      case "manual": {
        const why = reasonOf(await host.request({ kind: "mark_manual", monitor: a!, discard_codes: false }));
        const s = host.snap();
        const home = s.kind === "connected" ? homeOf(o, s.state, a!) : null;
        // Sin saber dónde está, no se inventa: el relleno no cuenta como dicho por el usuario,
        // y si luego se calibra, el paso vuelve a preguntar «¿qué ves ahora?».
        const entry: VerifyEntry = home
          ? { home, phase: { kind: "manual", reason: "sin perfil DDC" } }
          : { home: "mac", unknownHome: true, phase: { kind: "manual", reason: "sin perfil DDC" } };
        return set({ ...o, error: why, verify: { ...o.verify, [a!]: entry } });
      }
      case "verify":
        return verify(a!, { type: "start" });
      case "ask":
        return verify(a!, { type: "answer", yes: c === "yes" });
      case "calibrate":
        return void calibrate(a!);
      case "mode":
        return set({ ...o, initial: { ...o.initial, mode: a as ModeKey, applied: false, looksRight: null } });
      case "apply": {
        const s = host.snap();
        if (s.kind !== "connected" || !o.initial.mode) return;
        set({ ...o, initial: { ...o.initial, applying: true, applied: false, looksRight: null } });
        const r = await host.request({ kind: "apply_mode", mode: modeRequest(o.initial.mode, s.desk, s.state) });
        if (r.kind === "accepted") waiting = { txn: r.txn, then: "apply" };
        else set({ ...o, error: reasonOf(r), initial: { ...o.initial, applying: false } });
        return;
      }
      case "looks":
        return set({ ...o, initial: { ...o.initial, looksRight: a === "yes" } });
      case "finish":
        dialog.close();
        host.message("Listo: el puesto quedó configurado");
        return;
    }
  }

  async function pressKey(what: string, machine?: string) {
    if (what === "start") {
      set({ ...o, keyboard: { ...o.keyboard, armed: true, heard: null } });
      const why = reasonOf(await host.request({ kind: "key_probe", on: true }));
      if (why) set({ ...o, error: why });
    } else if (what === "again") {
      set({ ...o, keyboard: { ...o.keyboard, armed: true, heard: null, confirmed: null, undefinedOk: false } });
      void host.request({ kind: "key_probe", on: true });
    } else if (what === "yes" || what === "pick") {
      const m = machine as Machine;
      const why = reasonOf(await host.request({ kind: "set_keyboard", machine: m }));
      if (why) return set({ ...o, error: why });
      void host.request({ kind: "key_probe", on: false });
      set({ ...o, error: null, keyboard: { armed: false, heard: null, confirmed: m, undefinedOk: false } });
    } else if (what === "skip") {
      void host.request({ kind: "key_probe", on: false });
      set({ ...o, keyboard: { ...o.keyboard, armed: false, heard: null, undefinedOk: true } });
    }
  }

  // ---- Dibujo

  function checkEl(c: Check) {
    const li = el("li", `ob-check ${c.state}`);
    li.append(el("i", undefined, MARK[c.state]));
    const text = el("span");
    text.append(el("b", undefined, c.label));
    if (c.detail) text.append(el("small", undefined, c.detail));
    li.append(text);
    return li;
  }

  function buttonEl(b: Btn) {
    const node = el("button", `kbtn${b.kind ? ` ${b.kind}` : ""}`, b.label);
    node.type = "button";
    node.disabled = !b.enabled;
    node.dataset.id = b.id;
    if (b.hint) node.title = b.hint;
    node.addEventListener("click", () => void press(b));
    return node;
  }

  function rowEl(r: Row) {
    const row = el("div", "ob-row");
    row.dataset.key = r.key;
    const head = el("div", "ob-rhead");
    head.append(el("b", undefined, r.title), el("small", undefined, r.subtitle));
    row.append(head);
    const list = el("ul", "ob-checks");
    list.append(...r.checks.map(checkEl));
    row.append(list);
    for (const q of r.questions) {
      const box = el("div", "ob-q");
      box.append(el("p", undefined, q.text));
      const opts = el("div", "ob-opts");
      opts.setAttribute("role", "radiogroup");
      for (const opt of q.options) {
        const b = el("button", `kbtn${opt.selected ? " sel" : ""}`, opt.label);
        b.type = "button";
        b.setAttribute("role", "radio");
        b.setAttribute("aria-checked", String(opt.selected));
        b.dataset.id = opt.id;
        b.addEventListener("click", () => void press({ id: opt.id, label: opt.label, enabled: true }));
        opts.append(b);
      }
      box.append(opts);
      row.append(box);
    }
    if (r.buttons.length) {
      const btns = el("div", "ob-rbtns");
      for (const b of r.buttons) {
        const node = buttonEl(b);
        // «add:<clave>» es una casilla: se ve marcada cuando se agregará.
        if (b.id.startsWith("add:")) {
          node.classList.toggle("sel", b.label === "Se agregará");
          node.setAttribute("aria-pressed", String(b.label === "Se agregará"));
        }
        btns.append(node);
      }
      row.append(btns);
    }
    return row;
  }

  function render() {
    if (!dialog.open) return;
    const snap = host.snap();
    const v: StepView = viewOf(o, snap);
    const at = indexOf(o.step);
    dots.replaceChildren(
      ...STEPS.map((s, i) => {
        const d = el("span", i < at ? "done" : i === at ? "on" : undefined);
        d.title = s.title;
        return d;
      }),
    );
    const parts: Node[] = [];
    const h = el("h2", undefined, `${at + 1}. ${v.title}`);
    h.id = "onboard-title";
    parts.push(h, el("p", "ahint", v.lead));
    if (v.code) {
      const code = el("div", "ob-code", v.code.replace(/(\d{3})(\d{3})/, "$1 $2"));
      code.setAttribute("aria-label", `Código ${v.code.split("").join(" ")}`);
      parts.push(code);
    }
    if (v.checks.length) {
      const list = el("ul", "ob-checks");
      list.append(...v.checks.map(checkEl));
      parts.push(list);
    }
    parts.push(...v.rows.map(rowEl));
    if (v.buttons.length) {
      const box = el("div", "ob-btns");
      box.append(...v.buttons.map(buttonEl));
      parts.push(box);
    }
    if (o.error) parts.push(el("p", "pwarn", o.error));
    body.replaceChildren(...parts);

    const back = el("button", "kbtn", "Atrás");
    back.type = "button";
    back.disabled = o.step === "machine";
    back.addEventListener("click", () => go(goBack(o, host.snap())));
    const later = el("button", "kbtn", "Más tarde");
    later.type = "button";
    later.addEventListener("click", () => dialog.close());
    const next = el("button", "kbtn primary", "Siguiente");
    next.type = "button";
    next.id = "onboard-next";
    next.disabled = !canAdvance(o, snap);
    next.title = v.blocker ?? "";
    next.addEventListener("click", () => go(advanceTo(o, host.snap())));
    const why = el("span", "ob-why", v.blocker ?? (o.step === "summary" ? "" : "Verificado"));
    foot.replaceChildren(back, later, el("span", "spacer"), why, ...(o.step === "summary" ? [] : [next]));
    syncPolling();
  }

  function go(next: Onboarding) {
    if (next.step !== o.step) disarmKeys();
    set(next.step !== o.step ? { ...next, keyboard: next.step === "keyboard" ? next.keyboard : { ...next.keyboard, armed: false } } : next);
  }

  // ---- Eventos de afuera

  addEventListener("keydown", (e) => {
    if (!dialog.open || o.step !== "keyboard" || !o.keyboard.armed || o.keyboard.heard) return;
    if (!countsAsKeyProbe({ key: e.key, repeat: e.repeat, targetIsButton: e.target instanceof HTMLButtonElement })) return;
    void host.request({ kind: "heard_key" });
  });

  dialog.addEventListener("close", () => {
    // Cerrar a mitad de una prueba: lo que espera respuesta vuelve ya; lo que todavía se está
    // moviendo vuelve en cuanto termine el movimiento.
    abandoned = abandonOnClose(o.verify, abandoned);
    deadline.expireAll();
    disarmKeys();
    set({ ...o, keyboard: { ...o.keyboard, armed: false } });
    syncPolling();
    // Cerrarlo a mano es no querer verlo de nuevo solo, salvo que el puesto siga vacío y
    // se vuelva a abrir desde el botón.
    dismissed = true;
  });

  return {
    open() {
      o = begin();
      abandoned = keepInFlight(abandoned, waiting?.then === "move" ? waiting.monitor : null);
      opened = true;
      if (!dialog.open) dialog.showModal();
      render();
    },
    isOpen: () => dialog.open,
    maybeOpen(snap) {
      if (opened || dismissed || !shouldOpen(snap)) return;
      this.open();
    },
    onSnapshot() {
      render();
    },
    onNotice(n) {
      if (n.kind === "key_probe") {
        // También lo arma el asistente de la otra máquina: se refleja en el paso.
        set({ ...o, keyboard: { ...o.keyboard, armed: n.event.armed, heard: n.event.heard } });
        return;
      }
      if (n.kind === "txn" && "txn" in n.event) {
        const monitor = returning.get(n.event.txn);
        if (monitor !== undefined && n.event.type !== "started") {
          if (n.event.type === "rolled_back" || n.event.type === "inconsistent") {
            returning.delete(n.event.txn);
            host.message(`No se pudo devolver el ${monitor} a donde estaba: el cambio se deshizo. Revisa el monitor.`);
          } else if (n.event.type === "committed") returning.delete(n.event.txn);
        }
      }
      if (n.kind !== "txn" || !waiting) return;
      const e = n.event;
      if (e.type === "committed" && e.txn === waiting.txn) {
        const w = waiting;
        waiting = null;
        // Cerrado a mitad del movimiento: vuelve directo, sin pasar por la prueba.
        if (w.then === "move" && settle(w.monitor, "committed")) return;
        if (w.then === "move") verify(w.monitor, { type: "moved", ok: true });
        else set({ ...o, initial: { ...o.initial, applying: false, applied: true } });
      } else if ((e.type === "rolled_back" || e.type === "inconsistent") && e.txn === waiting.txn) {
        const w = waiting;
        waiting = null;
        const reason = e.type === "rolled_back" ? "el cambio no se pudo hacer y se deshizo" : "no se pudo deshacer todo: revisa los monitores";
        if (w.then === "move") {
          settle(w.monitor, e.type);
          verify(w.monitor, { type: "moved", ok: false, reason });
        }
        else set({ ...o, error: reason, initial: { ...o.initial, applying: false } });
      }
    },
  };
}
