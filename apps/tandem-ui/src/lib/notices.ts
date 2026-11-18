// Avisos de la app (F-Avisos): qué cambio del puesto merece un aviso y con qué texto. Puro:
// el store lo llama con el estado de antes y el de ahora.

import type { Machine } from "./model";
import type { DeskChange, DeskView, LinkKind, TransferView } from "./store/types";

export type ToastTone = "ok" | "bad" | "warn" | "info" | "claude";

/** Color del punto de cada tono (`F-Avisos`). */
export const TONE_COLOR: Record<ToastTone, string> = { ok: "#6cc59a", bad: "#e07a6f", warn: "#d9a75a", info: "#7aa7e6", claude: "#a99be8" };

export interface ToastAction {
  label: string;
  run: () => void;
}

/** Lo que pide un aviso; el store le pone el id y lo apaga a los 6 s si no pide una decisión. */
export interface ToastSpec {
  /** Con clave, un aviso nuevo reemplaza al que ya la tenga (el avance de una copia). */
  key?: string;
  /** Avance con barra: el aviso no se va solo hasta que termina. */
  progress?: { done: number; total: number };
  text: string;
  body?: string;
  tone: ToastTone;
  action?: ToastAction;
  /** No se va solo (el demo de `F-Avisos`, para capturar la pila entera). */
  hold?: boolean;
}

export const KIND_NAME: Record<LinkKind, string> = { usb4: "USB-C", ethernet: "Ethernet", wifi: "Wi-Fi" };

const nameOf = (d: DeskView, m: Machine) => d.machines[m]?.name ?? `la máquina ${m.toUpperCase()}`;

/**
 * Avisos de ejemplo del modo demo (`?demo&avisos`): los tres con que abre `F-Avisos`, del más nuevo
 * al más viejo. Solo se siembran a pedido, para ver y capturar la pila. El botón «Enviar por Wi-Fi igual» no hace nada: el agente no tiene esa función y la app real no lo muestra; está aquí para que la pila mida lo mismo que el mockup.
 */
export const DEMO_AVISOS: ToastSpec[] = [
  { text: "Se desconectó el cable USB-C", body: "El teclado y el mouse siguen por Wi-Fi. Los envíos de datos esperan al cable.", tone: "warn", action: { label: "Enviar por Wi-Fi igual", run: () => {} } },
  { text: "Escena «Trabajo» aplicada", body: '34" confirmado en PC-Escritorio en 1.8 s.', tone: "ok" },
  { text: "Claude abrió el túnel «tandem»", body: "MacBook Pro → PC-Escritorio · WSL.", tone: "claude" },
];

/** Bytes legibles: «1.4 GB», «320 MB», «12 KB». */
export function bytesText(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${Math.round(n / 1e6)} MB`;
  return `${Math.max(0, Math.round(n / 1e3))} KB`;
}

/** El aviso de una copia de archivos: con barra mientras avanza y «Listo» al terminar. */
export function transferToast(t: TransferView, d: DeskView): ToastSpec {
  const name = t.path.split(/[\\/]/).filter(Boolean).pop() ?? t.path;
  const peer = nameOf(d, d.here === "a" ? "b" : "a");
  const key = `transfer:${t.path}`;
  if (t.finished) return { key, text: `Se copió «${name}»`, body: t.upload ? `Ya está en ${peer}.` : `Ya está en esta máquina.`, tone: "ok" };
  const pct = t.total > 0 ? Math.min(100, Math.floor((t.done / t.total) * 100)) : 0;
  return {
    key,
    text: t.upload ? `Enviando «${name}» a ${peer}` : `Recibiendo «${name}» de ${peer}`,
    body: `${pct} % · ${bytesText(t.done)} de ${bytesText(t.total)}`,
    tone: "info",
    progress: { done: t.done, total: t.total },
  };
}

/** «Cambiado desde X · Deshacer» (D5): el aviso espera la decisión, no se va solo. */
export function changeToast(c: DeskChange, d: DeskView, undo: (rev: number) => void): ToastSpec {
  return {
    text: `Cambiado desde ${nameOf(d, c.by)}`,
    body: c.summary,
    tone: "info",
    action: { label: "Deshacer", run: () => undo(c.undoTo) },
  };
}

/** Los avisos que salen de pasar de `before` a `after` (cambio de camino, túnel, máquina). */
export function deskEvents(before: DeskView, after: DeskView): ToastSpec[] {
  if (!before.ready || !after.ready) return [];
  const out: ToastSpec[] = [];

  const was = before.link.kind;
  const now = after.link.kind;
  if (was && now && was !== now) {
    if (now === "wifi") {
      out.push({ text: `Se desconectó el cable ${KIND_NAME[was]}`, body: "El teclado y el mouse siguen por Wi-Fi. Los envíos de datos esperan al cable.", tone: "warn" });
    } else {
      out.push({ text: `Volvió el cable ${KIND_NAME[now]}`, body: "La entrada y los envíos vuelven a ir por el cable.", tone: "ok" });
    }
  }

  const known = new Set(before.tunnels.map((t) => t.id));
  for (const t of after.tunnels) {
    if (known.has(t.id) || t.state === "closed") continue;
    out.push({ text: `Claude abrió el túnel «${t.title}»`, body: `${nameOf(after, t.from)} → ${nameOf(after, t.to)}`, tone: "claude" });
  }

  const peer: Machine = after.here === "a" ? "b" : "a";
  const p0 = before.machines[peer]?.power;
  const p1 = after.machines[peer]?.power;
  if (p0 && p1 && p0 !== p1) {
    if (p1 === "on") out.push({ text: `${nameOf(after, peer)} volvió`, body: "Encendida y otra vez en el puesto.", tone: "claude" });
    else if (p0 === "on" && (p1 === "off" || p1 === "lost")) {
      out.push({ text: `${nameOf(after, peer)} ${p1 === "off" ? "se apagó" : "se perdió"}`, body: "El mouse ya no cruza hacia ella.", tone: "warn" });
    }
  }
  return out;
}
