// La fuente real: el estado de tandemd por la IPC (a través de Tauri). Traduce el
// `Snapshot` del agente al `DeskView` de las pantallas.
//
// Hoy la IPC manda una foto completa con cada cambio: `watch` solo recuerda qué temas pide
// la pantalla visible. Los eventos por tema (recursos, túneles, módulos) llegan con
// T-098/T-102/T-104 y se suscriben aquí.

import type { Bridge } from "../bridge";
import { machineColor, machineName, other } from "../machines";
import type { Caps, Link, Machine, MachineCaps, Snapshot } from "../model";
import type { ActResult, DeskView, MachineView, Source, Topic } from "./types";

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
};

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

/** El puesto que dibuja la UI a partir de la foto del agente. Puro. */
export function deskFromSnapshot(snap: Snapshot): DeskView {
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
  };
}

export class LiveSource implements Source {
  private topics: Topic[] = [];
  private last: Snapshot = { kind: "connecting" };

  constructor(private readonly bridge: Bridge) {}

  async start(onDesk: (d: DeskView) => void): Promise<void> {
    const push = (s: Snapshot) => {
      this.last = s;
      onDesk(deskFromSnapshot(s));
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

  can(what: "scenes" | "modules" | "power" | "measure"): boolean {
    if (what === "power" || what === "measure") return this.last.kind === "connected" && this.last.peer.kind === "paired";
    return false;
  }

  stop(): void {}
}
