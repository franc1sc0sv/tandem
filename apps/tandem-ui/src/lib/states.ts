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
export type StateDo = "add-machine" | "power-on" | "retry-scene" | "tunnels" | "permission";

export interface StateAction {
  label: string;
  icon: IconName;
  variant: "pri" | "default" | "ghost";
  does: StateDo;
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

/** Las nueve tarjetas, en el orden del mockup. */
export function stateCards(d: DeskView): StateCardSpec[] {
  const here = d.machines[d.here]?.name ?? "Esta máquina";
  const peerId = d.here === "a" ? "b" : "a";
  const peer = d.machines[peerId]?.name ?? "La otra máquina";
  const mac = d.machines[d.here]?.os === "macos";
  const owner = d.tunnels[0]?.title ?? "Entrenar LoRA";
  return [
    {
      id: "first",
      tag: "Primera vez",
      tone: "blue",
      icon: "plus",
      title: "Puesto de una sola máquina",
      body: "Todo funciona en esta máquina. Lo que necesita otra (teclado compartido, cómputo, escenas) aparece apagado con «Agrega otra máquina para usar esto».",
      action: { label: "Agregar máquina", icon: "plus", variant: "pri", does: "add-machine" },
    },
    {
      id: "off",
      tag: "Máquina apagada",
      tone: "grey",
      icon: "power",
      stroke: 2.2,
      title: `${peer} está apagada`,
      body: `Su tarjeta queda atenuada y el mouse ya no cruza hacia ella. ${here} puede despertarla por la red.`,
      action: { label: "Encender", icon: "power", variant: "pri", does: "power-on", hint: WAKE_HINT },
    },
    {
      id: "cable",
      tag: "Enlace",
      tone: "amber",
      icon: "wifi-fill",
      title: "Se desconectó el cable USB-C",
      body: "El teclado y el mouse siguieron por Wi-Fi sin cortarse (0.6 s). El envío de 2 workspaces quedó en pausa hasta que vuelva un cable, o puedes mandarlos por Wi-Fi ahora.",
    },
    {
      id: "resumed",
      tag: "Cómputo",
      tone: "blue",
      icon: "sync",
      title: "Reconectado: el túnel siguió",
      body: `«${owner}» siguió corriendo en ${peer} mientras no había enlace. Se recuperaron 3 min de salida.`,
      mono: "epoch 9/20 · loss 0.0371",
    },
    {
      id: "failed",
      tag: "Cómputo",
      tone: "red",
      icon: "x",
      title: "«Render ComfyUI» falló",
      body: `Sin memoria de GPU: «${owner}» usa 14.1 de 16 GB en ${peer}. Claude ve este motivo por MCP y puede esperar o elegir otra máquina.`,
      action: { label: "Ver en Túneles", icon: "eye", variant: "ghost", does: "tunnels" },
    },
    {
      id: "scene",
      tag: "Monitores",
      tone: "red",
      icon: "screen",
      title: "La escena «Solo MacBook» no se aplicó",
      body: "El 34\" respondió «cambio fallido» (no hay señal HDMI de la MacBook). Tandem deshizo la escena: todo quedó como estaba.",
      action: { label: "Reintentar", icon: "refresh", variant: "default", does: "retry-scene" },
    },
    {
      id: "unsupported",
      tag: "Monitores",
      tone: "amber",
      icon: "warn",
      title: "Monitor no compatible",
      body: "Tandem solo controla monitores que cumplen el estándar: dicen qué entrada muestran, confirman cada cambio y avisan los cambios hechos con sus botones. Los demás se ven, pero no se tocan.",
    },
    {
      id: "nobody",
      tag: "Monitores",
      tone: "grey",
      icon: "screen",
      title: "Nadie puede hablar con el 34\"",
      body: `Su dueño (${peer}) está apagado y ${here} solo llega a él por HDMI mientras lo está mostrando. Cuando una de las dos pueda, toma el control sola.`,
    },
    {
      id: "permission",
      tag: "Permisos",
      tone: "amber",
      icon: "shield-plain",
      title: mac ? "Falta «Monitoreo de entrada» en macOS" : "Falta un permiso del sistema",
      body: mac
        ? `Sin este permiso, ${here} no puede mandar su teclado. Concédelo a {code}; Tandem lo detecta solo.`
        : `Sin este permiso, ${here} no puede mandar su teclado. Concédelo a Tandem; Tandem lo detecta solo.`,
      code: mac ? (d.permissions.binary ?? "/Applications/Tandem.app/…/tandemd") : undefined,
      action: { label: "Abrir Ajustes del Sistema", icon: "external", variant: "pri", does: "permission" },
    },
  ];
}
