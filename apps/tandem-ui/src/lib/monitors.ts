// Los monitores de la pantalla Monitores (`V3-Monitores`): del JSON del agente (`MonitorView`)
// a lo que se dibuja. Puro. D11: un monitor que no habla el estándar se muestra «no
// compatible» con su razón y nunca se le pide nada.

import { wallOf } from "./acomodo";
import { MACHINES } from "./machines";
import type { Machine, MonitorEventJson, MonitorInput, MonitorJson } from "./model";
import type { MonitorEventView, MonitorUi } from "./store/types";

export const INPUT_LABEL: Record<MonitorInput, string> = { display_port: "DP", hdmi: "HDMI" };

/** Cuántos eventos se listan en la tarjeta (los más nuevos). */
export const EVENTS_SHOWN = 3;

/** El nombre corto de un monitor para frases («34"» de `Odyssey G5 34"`). */
export function shortMonitor(name: string): string {
  return /\d+(?:[.,]\d+)?"/.exec(name)?.[0] ?? name;
}

// Los dos acentos por omisión tienen el tono exacto del mockup (borde del indicador); el
// resto se oscurece igual.
const ACCENT: Record<string, string> = { "#7aa7e6": "#4f7cbc", "#a99be8": "#6a5fa6" };

/** El borde del indicador deslizante cuando una máquina está elegida. */
export function accentOf(color: string): string {
  const known = ACCENT[color.toLowerCase()];
  if (known) return known;
  const m = /^#([0-9a-f]{6})$/i.exec(color);
  if (!m) return "#3a3d45";
  const n = parseInt(m[1], 16);
  const ch = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => Math.round(v * 0.7).toString(16).padStart(2, "0"));
  return `#${ch.join("")}`;
}

/** `HH:MM` en la hora local (la del reloj de la máquina dueña, ya convertida por el agente). */
export function formatClock(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** El ícono y el texto de un evento del firmware. */
export function eventView(e: MonitorEventJson): MonitorEventView {
  const to = e.input ? INPUT_LABEL[e.input] : null;
  const base = { at: e.at_ms };
  switch (e.kind) {
    case "SwitchRequested":
      return { ...base, kind: "switch", text: to ? `Entrada → ${to}, pedida por Tandem` : "Cambio de entrada pedido por Tandem" };
    case "Input":
      return { ...base, kind: "button", text: to ? `Entrada → ${to}, con los botones` : "Entrada cambiada con los botones" };
    case "SwitchDone":
      return { ...base, kind: "switch", text: to ? `Cambio a ${to} confirmado` : "Cambio de entrada confirmado" };
    case "SwitchFailed":
      return { ...base, kind: "failed", text: to ? `Cambio a ${to} sin imagen` : "Cambio de entrada sin imagen" };
    case "Signal":
      return { ...base, kind: "signal", text: to ? `Señal ${to} detectada` : "Señal detectada" };
    case "Boot":
      return { ...base, kind: "boot", text: "El monitor arrancó" };
    case "Power":
      return { ...base, kind: "power", text: "Cambió el encendido" };
    case "Setting":
      return { ...base, kind: "other", text: "Cambió un ajuste del menú" };
    case "Lost":
      return { ...base, kind: "other", text: "Se perdieron eventos" };
    case "Other":
      return { ...base, kind: "other", text: "Evento del monitor" };
  }
}

export function monitorUi(m: MonitorJson): MonitorUi {
  const compat = "Compatible" in m.compat ? ({ ok: true } as const) : ({ ok: false, reason: m.compat.NotCompatible.reason } as const);
  const ports: MonitorUi["ports"] = {};
  for (const [who, input] of m.via) ports[who] = input;
  const input = m.report?.input ?? null;
  const shows: Machine | null = input ? (MACHINES.find((who) => ports[who] === input) ?? null) : null;
  const events = [...(m.report?.events ?? [])].reverse().slice(0, EVENTS_SHOWN).map(eventView);
  return { id: m.id, name: m.name, compat, connected: m.seen_by.length > 0, owner: m.owner, input, ports, shows, events };
}

/** Compatibles primero, como los muestra la pantalla; el orden del agente se respeta dentro de cada grupo. */
export function monitorsUi(list: MonitorJson[]): MonitorUi[] {
  const all = list.map(monitorUi);
  return [...all.filter((m) => m.compat.ok), ...all.filter((m) => !m.compat.ok)];
}

/** Por qué no se puede pedir el cambio a `who` (o `null` si sí se puede). */
export function whyNotSwitch(m: MonitorUi, who: Machine, name: string): string | null {
  if (!m.compat.ok) return `${m.name} no es compatible con el estándar de Tandem`;
  if (!m.connected) return `${m.name} está desconectado`;
  if (!m.ports[who]) return `Falta saber a qué entrada va ${name}`;
  return null;
}

/** El fondo de la vista previa de una escena (`F-NuevaEscena`): un poco más claro que el de la miniatura. */
export function previewWall(color: string): string {
  const known: Record<string, [string, string]> = { "#7aa7e6": ["#34496a", "#222d3d"], "#a99be8": ["#463d63", "#2a2539"] };
  const hit = known[color.toLowerCase()];
  if (hit) return `linear-gradient(160deg,${hit[0]},${hit[1]})`;
  return wallOf(color);
}

/** Fondo de «sin cambiar»: la franja de las pantallas que la escena no toca. */
export const KEEP_WALL = "repeating-linear-gradient(45deg,#1b1c21 0 8px,#15161a 8px 16px)";
