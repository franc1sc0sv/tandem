// Los estados difíciles (`V3-Estados`): cada uno dice qué pasó, qué hizo Tandem solo y qué
// puede hacer el usuario. Puro: arma las tarjetas con los nombres reales del puesto.

import type { IconName } from "../ui/icons";
import type { DeskView } from "./store/types";

/** Color del ícono de la tarjeta: fondo y trazo (`V3-Estados`). */
export type StateTone = "blue" | "grey" | "amber" | "red";

export const TONE_STYLE: Record<StateTone, { bg: string; fg: string }> = {
  blue: { bg: "#16202e", fg: "#7aa7e6" },
  grey: { bg: "#1b1c21", fg: "#9a9da6" },
  amber: { bg: "#1e1a12", fg: "#d9a75a" },
  red: { bg: "#25171a", fg: "#e07a6f" },
};

/** Qué hace el botón de una tarjeta; la pantalla lo conecta con el store. */
export type StateDo = "pair-peer" | "power-on" | "retry-scene" | "tunnels" | "permission";

export interface StateAction {
  label: string;
  icon: IconName;
  variant: "pri" | "default" | "ghost";
  does: StateDo;
  /** La escena que «Reintentar» reaplica: la que falló. */
  sceneId?: string;
  /** Cuánto tarda, junto al botón («≈ 40 s»). */
  hint?: string;
}

export interface StateCardSpec {
  id: string;
  tag: string;
  tone: StateTone;
  icon: IconName;
  /** Trazo más grueso que el de la mayoría (los íconos de acción). */
  stroke?: number;
  title: string;
  body: string;
  /** Una línea de salida del trabajo, en monoespaciada. */
  mono?: string;
  /** Texto en monoespaciada dentro del cuerpo, donde dice `{code}`. */
  code?: string;
  action?: StateAction;
}

/** Cuánto tarda en despertar la otra máquina (DESIGN §8). */
export const WAKE_HINT = "≈ 40 s";

/**
 * La ruta del binario sin el texto que la rodea («Concede el permiso a este binario: <ruta> (pid
 * 12)») y abreviada: `/Applications/Tandem.app/Contents/MacOS/tandemd` → `/Applications/Tandem.app/…/tandemd`.
 */
export function shortPath(text: string | null): string {
  const raw = (text ?? "/Applications/Tandem.app/Contents/MacOS/tandemd").replace(/^.*?:\s*/, "").replace(/\s*\(pid \d+\)$/, "");
  const parts = raw.split("/");
  return parts.length > 4 ? [...parts.slice(0, 3), "…", parts[parts.length - 1]].join("/") : raw;
}

/** De dónde salen las tarjetas: el estado real del puesto, o la galería completa del demo. */
export interface StateContext {
  /** Demo: se muestran las nueve, con datos genéricos. En la app real solo las que aplican. */
  gallery: boolean;
  /** La última escena que no se pudo aplicar (la guarda el store al ver el fallo). */
  failedScene: { id: string; name: string } | null;
  /** `?demo&estados`: los textos de ejemplo de `V3-Estados` (nombres, cifras), para compararlos con el mockup. */
  example?: boolean;
}

/** Los textos de ejemplo del mockup, por tarjeta: lo único que cambia respecto a la galería genérica. */
const EXAMPLE: Record<string, Partial<Pick<StateCardSpec, "title" | "body" | "mono" | "code">>> = {
  off: { title: "PC-Escritorio está apagada", body: "Su tarjeta queda atenuada y el mouse ya no cruza hacia ella. MacBook Pro puede despertarla por la red." },
  cable: {
    title: "Se desconectó el cable USB-C",
    body: "El teclado y el mouse siguieron por Wi-Fi sin cortarse (0.6 s). El envío de 2 workspaces quedó en pausa hasta que vuelva un cable, o puedes mandarlos por Wi-Fi ahora.",
  },
  resumed: { body: "«Entrenar LoRA» siguió corriendo en PC-Escritorio mientras no había enlace. Se recuperaron 3 min de salida.", mono: "epoch 9/20 · loss 0.0371" },
  failed: { title: "«Render ComfyUI» falló", body: "Sin memoria de GPU: «Entrenar LoRA» usa 14.1 de 16 GB en PC-Escritorio. Claude ve este motivo por MCP y puede esperar o elegir otra máquina." },
  scene: { title: "La escena «Solo MacBook» no se aplicó", body: "El 34\" respondió «cambio fallido» (no hay señal HDMI de la MacBook). Tandem deshizo la escena: todo quedó como estaba." },
  unsupported: { title: "Monitor no compatible" },
  nobody: { title: "Nadie puede hablar con el 34\"", body: "Su dueño (PC-Escritorio) está apagado y MacBook Pro solo llega a él por HDMI mientras lo está mostrando. Cuando una de las dos pueda, toma el control sola." },
  permission: { title: "Falta «Monitoreo de entrada» en macOS", body: "Sin este permiso, MacBook Pro no puede mandar su teclado. Concédelo a {code}; Tandem lo detecta solo.", code: "/Applications/Tandem.app/…/tandemd" },
};

/** Las tarjetas que aplican (o las nueve en la galería), en el orden del mockup. */
export function stateCards(d: DeskView, ctx: StateContext = { gallery: false, failedScene: null }): StateCardSpec[] {
  const here = d.machines[d.here]?.name ?? "Esta máquina";
  const peerId = d.here === "a" ? "b" : "a";
  const peer = d.machines[peerId]?.name ?? "La otra máquina";
  const mac = d.machines[d.here]?.os === "macos";
  const g = ctx.gallery;
  const alone = d.pair.peer.kind === "unpaired";
  const peerOff = !alone && d.machines[peerId]?.power === "off";
  // Sin cable: por Wi-Fi o, peor, sin ningún enlace (con la otra máquina apagada es la tarjeta de arriba).
  const noCable = !alone && !peerOff && d.link.kind !== "usb4" && d.link.kind !== "ethernet";
  // Los datos de la tarjeta salen del puesto; sin ellos (galería) se nombra lo genérico.
  const tunnel = d.tunnels.find((t) => t.state !== "closed") ?? d.tunnels[0];
  const job = tunnel ? `«${tunnel.title}»` : "El trabajo";
  const scene = ctx.failedScene ?? (g ? { id: d.activeScene ?? d.scenes[0]?.id ?? "", name: d.scenes.find((x) => x.id === d.activeScene)?.name ?? d.scenes[0]?.name ?? "" } : null);
  const bad = d.monitors.find((m) => !m.compat.ok);
  const unreachable = d.monitors.find((m) => m.compat.ok && m.owner === null);
  const monitor = unreachable ?? d.monitors.find((m) => m.compat.ok);
  const missingPermission = d.permissions.rows.some((r) => r.state === "missing") || d.trouble === "permissions";

  const cards: (StateCardSpec | null)[] = [
    alone || g
      ? {
          id: "first",
          tag: "Primera vez",
          tone: "blue",
          icon: "plus",
          title: "Puesto de una sola máquina",
          body: "Todo funciona en esta máquina. Lo que necesita otra (teclado compartido, cómputo, escenas) aparece apagado hasta que emparejes la otra máquina.",
          action: { label: "Emparejar la otra máquina", icon: "plus", variant: "pri", does: "pair-peer" },
        }
      : null,
    peerOff || g
      ? {
          id: "off",
          tag: "Máquina apagada",
          tone: "grey",
          icon: "power",
          stroke: 2.2,
          title: `${peer} está apagada`,
          body: `Su tarjeta queda atenuada y el mouse ya no cruza hacia ella. ${here} puede despertarla por la red.`,
          action: { label: "Encender", icon: "power", variant: "pri", does: "power-on", hint: WAKE_HINT },
        }
      : null,
    noCable || g
      ? {
          id: "cable",
          tag: "Enlace",
          tone: "amber",
          icon: "wifi-fill",
          title: "Se desconectó el cable",
          body:
            d.link.kind === null && !g
              ? `No hay enlace con ${peer}. El teclado y el mouse no cruzan hasta que vuelva un cable o el Wi-Fi.`
              : "El teclado y el mouse siguen por Wi-Fi. Los envíos de datos esperan a que vuelva un cable, o puedes mandarlos por Wi-Fi ahora.",
        }
      : null,
    // Reconexión y fallo de un trabajo: el agente todavía no manda ese historial, así que solo la galería.
    g
      ? {
          id: "resumed",
          tag: "Cómputo",
          tone: "blue",
          icon: "sync",
          title: "Reconectado: el túnel siguió",
          body: `${job} siguió corriendo en ${peer} mientras no había enlace. Se recuperó la salida que faltaba.`,
        }
      : null,
    g
      ? {
          id: "failed",
          tag: "Cómputo",
          tone: "red",
          icon: "x",
          title: `${job} falló`,
          body: `Se quedó sin recursos en ${peer}. Claude ve este motivo por MCP y puede esperar o elegir otra máquina.`,
          action: { label: "Ver en Túneles", icon: "eye", variant: "ghost", does: "tunnels" },
        }
      : null,
    scene && (ctx.failedScene || g)
      ? {
          id: "scene",
          tag: "Monitores",
          tone: "red",
          icon: "screen",
          title: `La escena «${scene.name}» no se aplicó`,
          body: "Un monitor respondió «cambio fallido». Tandem deshizo la escena: todo quedó como estaba.",
          action: { label: "Reintentar", icon: "refresh", variant: "default", does: "retry-scene", sceneId: scene.id },
        }
      : null,
    bad || g
      ? {
          id: "unsupported",
          tag: "Monitores",
          tone: "amber",
          icon: "warn",
          title: bad ? `«${bad.name}» no es compatible` : "Monitor no compatible",
          body: "Tandem solo controla monitores que cumplen el estándar: dicen qué entrada muestran, confirman cada cambio y avisan los cambios hechos con sus botones. Los demás se ven, pero no se tocan.",
        }
      : null,
    unreachable || g
      ? {
          id: "nobody",
          tag: "Monitores",
          tone: "grey",
          icon: "screen",
          title: `Nadie puede hablar con ${monitor ? `«${monitor.name}»` : "el monitor"}`,
          body: `Su dueño (${peer}) está apagado y ${here} solo llega a él por HDMI mientras lo está mostrando. Cuando una de las dos pueda, toma el control sola.`,
        }
      : null,
    missingPermission || g
      ? {
          id: "permission",
          tag: "Permisos",
          tone: "amber",
          icon: "shield-plain",
          title: mac ? "Falta «Monitoreo de entrada» en macOS" : "Falta un permiso del sistema",
          body: mac
            ? `Sin este permiso, ${here} no puede mandar su teclado. Concédelo a {code}; Tandem lo detecta solo.`
            : `Sin este permiso, ${here} no puede mandar su teclado. Concédelo a Tandem; Tandem lo detecta solo.`,
          code: mac ? shortPath(d.permissions.binary) : undefined,
          action: { label: "Abrir Ajustes del Sistema", icon: "external", variant: "pri", does: "permission" },
        }
      : null,
  ];
  return cards.filter((c): c is StateCardSpec => c !== null).map((c) => (ctx.example && EXAMPLE[c.id] ? { ...c, ...EXAMPLE[c.id] } : c));
}
