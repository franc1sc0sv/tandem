// «Puesto» (T-082): la configuración que el agente generó (máquina, monitores, cableado) y
// el cambio de cableado de un clic. Puro, para probarlo sin DOM.

import type { AgentSetup, Input, Machine, MonitorConfig } from "./model";

const SHORT: Record<Input, string> = { hdmi: "HDMI", displayport: "DP" };
const NAME: Record<Machine, string> = { mac: "Mac", pc: "PC" };
const OTHER_INPUT: Record<Input, Input> = { hdmi: "displayport", displayport: "hdmi" };

/** `DP → PC · HDMI → Mac`, la PC primero. */
export function wiringText(wiring: MonitorConfig["wiring"]): string {
  const parts = (["pc", "mac"] as Machine[]).flatMap((m) => {
    const input = wiring[m];
    return input ? [`${SHORT[input]} → ${NAME[m]}`] : [];
  });
  return parts.length ? parts.join(" · ") : "sin cablear";
}

/** El cableado al revés: cada máquina pasa a la otra entrada. */
export function swapWiring(wiring: MonitorConfig["wiring"]): Partial<Record<Machine, Input>> {
  const out: Partial<Record<Machine, Input>> = {};
  for (const m of ["pc", "mac"] as Machine[]) {
    const input = wiring[m];
    if (input) out[m] = OTHER_INPUT[input];
  }
  return out;
}

/** Lo que dice la cabecera del diálogo sobre la configuración. */
export function setupLines(agent: Machine, setup: AgentSetup | null): string[] {
  const lines = [`Esta máquina: ${agent === "mac" ? "la Mac" : "la PC"}.`];
  if (!setup) return lines;
  if (setup.generated) lines.push("Tandem detectó los monitores y escribió la configuración solo: no hace falta tocar ningún archivo.");
  if (setup.port !== null) lines.push(`El agente escucha en el puerto UDP ${setup.port}.`);
  return lines;
}

/** El modelo para mostrar debajo de la etiqueta (`SAM711A`), si no es la etiqueta misma. */
export function modelOf(m: MonitorConfig): string | null {
  const model = m.edid_model?.trim();
  return model && model !== m.label ? model : null;
}
