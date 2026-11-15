// El asistente de configuración inicial en pantalla (DESIGN §5): dibuja el `StepView` del
// paso actual y hace de puente entre sus botones y el agente. La lógica de qué se muestra y
// cuándo se avanza está en `onboarding.ts`, donde se prueba sin DOM.

import type { Bridge } from "./bridge";
import type { Action, ActionResult, Machine, Notice, Permission, Snapshot } from "./model";
import {
  STEPS,
  advanceTo,
  begin,
  canAdvance,
  countsAsKeyProbe,
  goBack,
  indexOf,
  shouldOpen,
  viewOf,
  type Btn,
  type Check,
  type Onboarding,
  type StepView,
} from "./onboarding";
import { pressPermission } from "./permissions";

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
  /** Abre solo en una instalación limpia y si no se descartó. */
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
  /** Se cerró a mano: no se vuelve a abrir solo en esta sesión. */
  let dismissed = false;
  let opened = false;

  const set = (next: Onboarding) => {
    o = next;
    render();
  };

  const disarmKeys = () => {
    if (o.keyboard.armed) void host.request({ kind: "key_probe", on: false });
  };

  async function press(b: Btn) {
    const [kind, a, c] = b.id.split(":");
    switch (kind) {
      case "perm": {
        // «Pedir permisos», «Reiniciar agente» y los paneles de Ajustes (T-090).
        const bridge = host.bridge();
        const r = await pressPermission(a!, {
          request: () => host.request({ kind: "request_permissions" }),
          restartAgent: bridge ? () => bridge.restartAgent() : null,
          openSettings: bridge ? () => bridge.openSettings(a as Permission) : null,
        });
        if (r.message) host.message(r.message);
        if (a === "request" || a === "restart") set({ ...o, error: r.error });
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
    back.addEventListener("click", () => go(goBack(o)));
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
  }

  function go(next: Onboarding) {
    if (next.step !== o.step) disarmKeys();
    set(next.step !== o.step ? { ...next, keyboard: next.step === "keyboard" ? next.keyboard : { ...next.keyboard, armed: false } } : next);
  }

  addEventListener("keydown", (e) => {
    if (!dialog.open || o.step !== "keyboard" || !o.keyboard.armed || o.keyboard.heard) return;
    if (!countsAsKeyProbe({ key: e.key, repeat: e.repeat, targetIsButton: e.target instanceof HTMLButtonElement })) return;
    void host.request({ kind: "heard_key" });
  });

  dialog.addEventListener("close", () => {
    disarmKeys();
    set({ ...o, keyboard: { ...o.keyboard, armed: false } });
    // Cerrarlo a mano es no querer verlo de nuevo solo; se vuelve a abrir desde el botón.
    dismissed = true;
  });

  return {
    open() {
      o = begin();
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
      }
    },
  };
}
