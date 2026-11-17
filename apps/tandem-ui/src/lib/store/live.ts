// La fuente real: el estado de tandemd por la IPC (a través de Tauri). Traduce el
// `Snapshot` del agente al `DeskView` de las pantallas.
//
// Hoy la IPC manda una foto completa con cada cambio: `watch` solo recuerda qué temas pide
// la pantalla visible. Los eventos por tema (recursos, túneles, módulos) llegan con
// T-098/T-102/T-104 y se suscriben aquí.

import type { Bridge } from "../bridge";
import { machineColor, machineName, other } from "../machines";
import type { ActionResult, Bindings, Caps, Link, Machine, MachineCaps, Permission, SetupView, ShortcutsView, Snapshot } from "../model";
import { grantTo, permissionState } from "../permissions";
import type { ActResult, DeskView, MachineView, PairActionKind, PermissionsView, Source, Topic } from "./types";

const MAC_PERMISSIONS: Permission[] = ["accessibility", "input_monitoring", "local_network"];

const OS_NAME = { macos: "macOS", windows: "Windows" } as const;

export const EMPTY_DESK: DeskView = {
  ready: false,
  trouble: "none",
  troubleText: "",
  here: "a",
  machines: { a: null, b: null },
  link: { kind: null, gbps: null, ms: null, inputP99: null },
  tunnels: [],
  scenes: [],
  activeScene: null,
  pair: { peer: { kind: "unpaired" }, pairing: { kind: "idle" } },
  permissions: { needed: false, rows: [], binary: null },
};

/** La foto del agente no tiene la forma que espera esta versión de la UI. */
export class ShapeError extends Error {
  constructor(what: string) {
    super(`el agente respondió algo que esta versión de la app no entiende (${what})`);
    this.name = "ShapeError";
  }
}

const isSide = (m: unknown): m is Machine => m === "a" || m === "b";
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

/**
 * Comprueba lo que la UI lee de una foto conectada. Un agente de otra versión (por ejemplo
 * uno con lados `mac`/`pc`) da un estado difícil con la razón, no una ventana colgada.
 */
export function checkSnapshot(snap: unknown): asserts snap is Snapshot {
  if (!isObj(snap) || typeof snap.kind !== "string") throw new ShapeError("sin «kind»");
  if (snap.kind !== "connected") return;
  if (!isSide(snap.agent)) throw new ShapeError(`lado «${String(snap.agent)}»`);
  const desk = snap.desk;
  if (!isObj(desk) || !isObj(desk.machines) || !Array.isArray(desk.screens) || (desk.arrangement !== null && !isObj(desk.arrangement))) throw new ShapeError("puesto");
  if (!isObj(snap.state) || !isObj(snap.state.on) || typeof snap.state.on.a !== "boolean" || typeof snap.state.on.b !== "boolean") throw new ShapeError("encendido");
  if (!isObj(snap.peer) || !isObj(snap.pairing)) throw new ShapeError("emparejamiento");
  if (!isObj(snap.input) || !Array.isArray(snap.input.missing)) throw new ShapeError("entrada");
  const caps = snap.caps;
  if (!isObj(caps) || !isObj(caps.here) || !isSide(caps.here.machine) || !isObj(caps.here.input)) throw new ShapeError("capacidades");
  if (caps.peer !== null && (!isObj(caps.peer) || !isSide(caps.peer.machine))) throw new ShapeError("capacidades de la otra");
}

function permissionsOf(here: MachineCaps): PermissionsView {
  if (here.os !== "macos") return { needed: false, rows: [], binary: null };
  return { needed: true, rows: MAC_PERMISSIONS.map((id) => ({ id, state: permissionState(here, id) })), binary: grantTo(here) };
}

function machineView(who: Machine, here: Machine, caps: Caps, snap: Extract<Snapshot, { kind: "connected" }>): MachineView | null {
  const c: MachineCaps | null = caps.here.machine === who ? caps.here : caps.peer?.machine === who ? caps.peer : null;
  const info = snap.desk.machines?.[who] ?? null;
  if (!c && !info) return null;
  const os = c?.os ?? info?.os ?? "windows";
  const on = snap.state.on[who];
  return {
    id: who,
    name: machineName(who, caps, snap.desk),
    os,
    color: machineColor(who, caps, snap.desk),
    detail: OS_NAME[os] ?? os,
    here: who === here,
    power: who === here || on ? "on" : "off",
    resources: null,
    modules: [],
  };
}

const LINK_KIND: Record<Link, DeskView["link"]["kind"]> = { usb4: "usb4", ethernet: "ethernet", wifi: "wifi" };

/** El puesto que dibuja la UI a partir de la foto del agente. Puro; lanza `ShapeError`. */
export function deskFromSnapshot(raw: Snapshot): DeskView {
  const snap: unknown = raw;
  checkSnapshot(snap);
  if (snap.kind === "connecting") return EMPTY_DESK;
  if (snap.kind === "unavailable") {
    return { ...EMPTY_DESK, ready: true, trouble: "no-agent", troubleText: snap.reason };
  }
  const here = snap.agent;
  const peer = other(here);
  const machines = { a: machineView("a", here, snap.caps, snap), b: machineView("b", here, snap.caps, snap) };
  const peerName = machineName(peer, snap.caps, snap.desk);
  const linked = snap.peer.kind === "paired" && snap.peer.link !== null;
  let trouble: DeskView["trouble"] = "none";
  let troubleText = "";
  if (snap.input.missing.length > 0) {
    trouble = "permissions";
    troubleText = "Faltan permisos en esta máquina";
  } else if (snap.peer.kind === "paired" && !snap.state.on[peer]) {
    trouble = "peer-off";
    troubleText = `${peerName} está apagada`;
  } else if (snap.peer.kind === "paired" && !linked) {
    trouble = "link-down";
    troubleText = `Sin enlace con ${peerName}`;
  }
  return {
    ...EMPTY_DESK,
    ready: true,
    trouble,
    troubleText,
    here,
    machines,
    link: { kind: snap.peer.kind === "paired" && snap.peer.link ? LINK_KIND[snap.peer.link] : null, gbps: null, ms: null, inputP99: null },
    pair: { peer: snap.peer, pairing: snap.pairing },
    permissions: permissionsOf(snap.caps.here),
  };
}

/** El estado difícil de un error al hablar con el agente. */
export function deskFromError(e: unknown): DeskView {
  return { ...EMPTY_DESK, ready: true, trouble: "no-agent", troubleText: e instanceof Error ? e.message : String(e) };
}

function result(r: ActionResult): ActResult {
  if (r.kind === "rejected" || r.kind === "unavailable") return { ok: false, reason: r.reason };
  return { ok: true };
}

export class LiveSource implements Source {
  private topics: Topic[] = [];
  private last: Snapshot = { kind: "connecting" };

  constructor(private readonly bridge: Bridge) {}

  async start(onDesk: (d: DeskView) => void): Promise<void> {
    // Un error al leer una foto no cuelga la ventana: se muestra como «sin agente» con la razón.
    const push = (s: Snapshot) => {
      try {
        const d = deskFromSnapshot(s);
        this.last = s;
        onDesk(d);
      } catch (e) {
        this.last = { kind: "connecting" };
        onDesk(deskFromError(e));
      }
    };
    await this.bridge.onSnapshot(push);
    push(await this.bridge.snapshot());
  }

  watch(topics: Topic[]): void {
    this.topics = [...topics];
  }

  get currentTopics(): Topic[] {
    return this.topics;
  }

  async applyScene(): Promise<ActResult> {
    return { ok: false, reason: "Las escenas todavía no están en el agente" };
  }

  async setPower(machine: Machine, on: boolean): Promise<ActResult> {
    const r = await this.bridge.act({ kind: "set_power", machine, on });
    if (r.kind === "accepted" || r.kind === "done") return { ok: true };
    return { ok: false, reason: "reason" in r ? r.reason : "El agente no lo aceptó" };
  }

  async measureLink(): Promise<ActResult> {
    const r = await this.bridge.act({ kind: "ping_peer" });
    if (r.kind === "pong") return { ok: true, text: `${r.millis} ms` };
    return { ok: false, reason: "reason" in r ? r.reason : "Sin respuesta de la otra máquina" };
  }

  async setModule(): Promise<ActResult> {
    return { ok: false, reason: "Los módulos todavía no están en el agente" };
  }

  async pairAction(kind: PairActionKind): Promise<ActResult> {
    return result(await this.bridge.act({ kind }));
  }

  async requestPermissions(): Promise<ActResult> {
    return result(await this.bridge.act({ kind: "request_permissions" }));
  }

  async openPermission(p: Permission): Promise<ActResult> {
    await this.bridge.openSettings(p);
    return { ok: true };
  }

  async restartAgent(): Promise<ActResult> {
    await this.bridge.restartAgent();
    return { ok: true, text: "tandemd se reinicia: en unos segundos se vuelven a comprobar los permisos" };
  }

  setup(): Promise<SetupView> {
    return this.bridge.setup();
  }

  async finishSetup(): Promise<void> {
    await this.bridge.finishSetup();
  }

  shortcuts(): Promise<ShortcutsView> {
    return this.bridge.shortcuts();
  }

  setShortcuts(b: Bindings): Promise<ShortcutsView> {
    return this.bridge.setShortcuts(b);
  }

  hideMain(): Promise<void> {
    return this.bridge.hideMain();
  }

  can(what: "scenes" | "modules" | "power" | "measure"): boolean {
    if (what === "power" || what === "measure") return this.last.kind === "connected" && this.last.peer.kind === "paired";
    return false;
  }

  stop(): void {}
}
