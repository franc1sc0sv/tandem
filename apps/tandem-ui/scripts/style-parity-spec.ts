// Paridad de estilos (T-127): qué componente de la galería (`gallery.html`, build demo) se
// compara con qué elemento de qué mockup, y el comparador de estilos calculados.
//
// La comparación por píxeles de `pnpm shots` mide pantallas enteras: un borde, una curva o una
// duración distinta en un botón casi no mueve píxeles (y una transición no se ve en una captura
// quieta). Aquí se comparan los valores de `getComputedStyle` del mismo Chromium, propiedad por
// propiedad. Este archivo no abre el navegador, así que se prueba con estilos de ejemplo.

/** Grupos de propiedades que se comparan (los nombres salen en el informe). */
export const GROUPS = {
  alto: ["height"],
  ancho: ["width"],
  relleno: ["padding-top", "padding-right", "padding-bottom", "padding-left"],
  fuente: ["font-family", "font-size", "font-weight", "color"],
  radio: ["border-top-left-radius", "border-top-right-radius", "border-bottom-right-radius", "border-bottom-left-radius"],
  borde: ["top", "right", "bottom", "left"].flatMap((s) => [`border-${s}-width`, `border-${s}-style`, `border-${s}-color`]),
  fondo: ["background-color", "background-image"],
  sombra: ["box-shadow"],
  posicion: ["top", "left", "bottom"],
  caja: ["box-sizing"],
  transition: ["transition-property", "transition-duration", "transition-timing-function", "transition-delay"],
  animation: ["animation-name", "animation-duration", "animation-timing-function", "animation-iteration-count", "animation-fill-mode"],
} as const satisfies Record<string, readonly string[]>;

export type Group = keyof typeof GROUPS;

/** Lo que pide la tarea para cada componente: alto, relleno, fuente, radio, borde, fondo, sombra y movimiento. */
export const ALL: Group[] = ["alto", "relleno", "fuente", "radio", "borde", "fondo", "sombra", "transition", "animation"];

/** Un elemento (o su `::after`) dentro de una página. */
export interface Target {
  selector: string;
  pseudo?: "::before" | "::after";
}

export interface StylePair {
  /** Nombre en el informe. */
  name: string;
  /** Elemento en `gallery.html`. */
  app: Target;
  /** Mockup (sin `.dc.html`) y elemento. */
  mockup: string;
  /** Estado con nombre del mockup (`tools/mockup-render/src/states.ts`) si el elemento no está en el initial. */
  state?: string;
  ref: Target;
  groups: Group[];
}

/** Valores calculados de un elemento: propiedad CSS → valor de `getComputedStyle`. */
export type Styles = Record<string, string>;

export interface StyleDiff {
  pair: string;
  group: Group;
  /** Propiedad CSS (o `transition: transform`, `animation`…). */
  property: string;
  app: string;
  mockup: string;
}

/** Todas las propiedades CSS que hay que leer para los grupos dados. */
export function propertiesOf(groups: readonly Group[]): string[] {
  return [...new Set(groups.flatMap((g) => GROUPS[g]))];
}

// ---- Normalización ------------------------------------------------------------------------

/** Separa una lista CSS por las comas de primer nivel (no las de `cubic-bezier(…)` ni `rgba(…)`). */
export function splitList(value: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of value) {
    if (ch === "(") depth++;
    else if (ch === ")") depth--;
    if (ch === "," && depth === 0) {
      out.push(cur.trim());
      cur = "";
    } else cur += ch;
  }
  if (cur.trim() !== "" || out.length > 0) out.push(cur.trim());
  return out;
}

/** Tiempo CSS en milisegundos (`0.18s`, `180ms`). */
export function toMs(value: string): number {
  const v = value.trim();
  if (v.endsWith("ms")) return Number.parseFloat(v);
  if (v.endsWith("s")) return Number.parseFloat(v) * 1000;
  return Number.parseFloat(v);
}

const NAMED_CURVES: Record<string, string> = {
  linear: "cubic-bezier(0, 0, 1, 1)",
  ease: "cubic-bezier(0.25, 0.1, 0.25, 1)",
  "ease-in": "cubic-bezier(0.42, 0, 1, 1)",
  "ease-out": "cubic-bezier(0, 0, 0.58, 1)",
  "ease-in-out": "cubic-bezier(0.42, 0, 0.58, 1)",
};

/** Curva en forma canónica: los nombres pasan a `cubic-bezier` y los números se redondean. */
export function normalizeCurve(value: string): string {
  const v = value.trim();
  return normalizeNumbers(NAMED_CURVES[v] ?? v);
}

/** Redondea los números de un valor (los cálculos de Chromium dejan `37.9999px`) y une espacios. */
export function normalizeNumbers(value: string): string {
  return value
    .replace(/-?\d*\.?\d+(?:e-?\d+)?/gi, (n) => {
      const r = Math.round(Number.parseFloat(n) * 100) / 100;
      return String(Object.is(r, -0) ? 0 : r);
    })
    .replace(/\s+/g, " ")
    .trim();
}

/** Solo la primera familia: es la que se dibuja (el resto es respaldo del sistema). */
export function firstFamily(value: string): string {
  return (splitList(value)[0] ?? "").replace(/["']/g, "").trim().toLowerCase();
}

/** El elemento `i` de una lista CSS que se repite si es más corta (así lo hace el navegador). */
function cycle(list: string[], i: number): string {
  return list.length === 0 ? "" : (list[i % list.length] ?? "");
}

/**
 * Transiciones de un elemento como `propiedad → «duración curva retraso»`. Las de duración y
 * retraso 0 (el valor initial `all 0s ease 0s`) no animan nada y se omiten.
 */
export function transitions(s: Styles): Map<string, string> {
  const props = splitList(s["transition-property"] ?? "");
  const durs = splitList(s["transition-duration"] ?? "");
  const curves = splitList(s["transition-timing-function"] ?? "");
  const delays = splitList(s["transition-delay"] ?? "");
  const out = new Map<string, string>();
  props.forEach((p, i) => {
    const d = toMs(cycle(durs, i) || "0s");
    const delay = toMs(cycle(delays, i) || "0s");
    if (p === "none" || (d === 0 && delay === 0)) return;
    out.set(p, `${d}ms ${normalizeCurve(cycle(curves, i) || "ease")}${delay ? ` +${delay}ms` : ""}`);
  });
  return out;
}

/**
 * Animaciones como lista de `«duración curva repeticiones relleno»`. El nombre no se compara
 * (Svelte le agrega un prefijo por componente) ni el retraso (las tarjetas entran escalonadas
 * con un `animation-delay` online): solo si hay animación, su duración y su curva.
 */
export function animations(s: Styles): string[] {
  const names = splitList(s["animation-name"] ?? "none");
  const durs = splitList(s["animation-duration"] ?? "");
  const curves = splitList(s["animation-timing-function"] ?? "");
  const counts = splitList(s["animation-iteration-count"] ?? "");
  const fills = splitList(s["animation-fill-mode"] ?? "");
  return names.flatMap((n, i) =>
    n === "none" ? [] : [`${toMs(cycle(durs, i) || "0s")}ms ${normalizeCurve(cycle(curves, i) || "ease")} ×${cycle(counts, i) || "1"} ${cycle(fills, i) || "none"}`],
  );
}

// ---- Comparación --------------------------------------------------------------------------

/** Si el lado de `border-<lado>-color` se dibuja: el color de un borde que no existe no se ve. */
function hasBorder(s: Styles, colorProperty: string): boolean {
  const side = colorProperty.replace(/-color$/, "");
  const style = s[`${side}-style`] ?? "none";
  return style !== "none" && style !== "hidden" && Number.parseFloat(s[`${side}-width`] ?? "0") > 0;
}

/** Compara dos elementos en los grupos pedidos. Devuelve solo lo que difiere. */
export function compareStyles(pair: string, app: Styles, mockup: Styles, groups: readonly Group[]): StyleDiff[] {
  const diffs: StyleDiff[] = [];
  for (const group of groups) {
    if (group === "transition") {
      const a = transitions(app);
      const m = transitions(mockup);
      for (const p of new Set([...a.keys(), ...m.keys()])) {
        if (a.get(p) !== m.get(p)) diffs.push({ pair, group, property: `transition: ${p}`, app: a.get(p) ?? "sin transición", mockup: m.get(p) ?? "sin transición" });
      }
      continue;
    }
    if (group === "animation") {
      const a = animations(app).join(", ") || "ninguna";
      const m = animations(mockup).join(", ") || "ninguna";
      if (a !== m) diffs.push({ pair, group, property: "animation", app: a, mockup: m });
      continue;
    }
    for (const property of GROUPS[group]) {
      if (group === "borde" && property.endsWith("-color") && !hasBorder(app, property) && !hasBorder(mockup, property)) continue;
      const norm = property === "font-family" ? firstFamily : normalizeNumbers;
      const a = norm(app[property] ?? "");
      const m = norm(mockup[property] ?? "");
      if (a !== m) diffs.push({ pair, group, property, app: a, mockup: m });
    }
  }
  return diffs;
}

/** Dónde está cada lado de un par, para el informe. */
export function where(p: StylePair): { app: string; mockup: string } {
  const t = (x: Target) => `${x.selector}${x.pseudo ?? ""}`;
  return { app: `gallery.html ${t(p.app)}`, mockup: `${p.mockup}${p.state ? ` (estado ${p.state})` : ""} ${t(p.ref)}` };
}

/** Informe legible: una línea por propiedad distinta, con el par y dónde está cada lado. */
export function formatReport(pairs: readonly StylePair[], diffs: readonly StyleDiff[], missing: readonly string[] = []): string {
  const lines: string[] = [];
  for (const p of pairs) {
    const mine = diffs.filter((d) => d.pair === p.name);
    const lost = missing.filter((m) => m.startsWith(`${p.name}:`));
    if (mine.length === 0 && lost.length === 0) {
      lines.push(`✓ ${p.name}`);
      continue;
    }
    const w = where(p);
    lines.push(`✗ ${p.name}  (app: ${w.app} · mockup: ${w.mockup})`);
    for (const l of lost) lines.push(`    ! ${l.slice(p.name.length + 1).trim()}`);
    for (const d of mine) lines.push(`    ${d.group} · ${d.property}: app «${d.app}» ≠ mockup «${d.mockup}»`);
  }
  return lines.join("\n");
}

// ---- Pares --------------------------------------------------------------------------------

const part = (name: string, inner: string, pseudo?: Target["pseudo"]): Target => ({ selector: `[data-part="${name}"] ${inner}`, ...(pseudo ? { pseudo } : {}) });
const ref = (selector: string, pseudo?: Target["pseudo"]): Target => ({ selector, ...(pseudo ? { pseudo } : {}) });

/** Lo que se pinta del brillo del primario (`::after`): fondo, posición y su transición. */
const SHINE: Group[] = ["fondo", "transition"];
/** El interruptor no tiene texto (el nombre va en `aria-label`): no se compara la fuente. */
const SWITCH: Group[] = [...ALL.filter((g) => g !== "fuente"), "ancho"];
/** La perilla del interruptor (`::after`). */
const KNOB: Group[] = ["alto", "ancho", "radio", "fondo", "posicion", "transition"];
/**
 * El indicador del segmentado. El ancho depende del ancho del grupo (otro en la galería), así
 * que se mira la caja (`box-sizing`): con la misma fórmula, la misma caja da el mismo ancho.
 */
const IND: Group[] = ["alto", "caja", "radio", "borde", "fondo", "sombra", "posicion", "transition"];

export const PAIRS: StylePair[] = [
  // V3-Arrangement: los botones del encabezado (38 px).
  { name: "botón 38 px", app: part("btn-38", ".btn"), mockup: "V3-Arrangement", ref: ref('a.btn[href="F-CrossingTest.dc.html"]'), groups: ALL },
  { name: "botón primario 38 px", app: part("btn-pri-38", ".btn"), mockup: "V3-Arrangement", ref: ref("button.btn.pri"), groups: ALL },
  { name: "brillo del primario", app: part("btn-pri-38", ".btn", "::after"), mockup: "V3-Arrangement", ref: ref("button.btn.pri", "::after"), groups: SHINE },
  // V3-Monitors: sm, primario sm y fantasma de solo ícono.
  { name: "botón sm", app: part("btn-sm", ".btn"), mockup: "V3-Monitors", ref: ref('a.btn.sm[href="V3-States.dc.html"]'), groups: ALL },
  { name: "botón primario sm", app: part("btn-pri-sm", ".btn"), mockup: "V3-Monitors", ref: ref("a.btn.pri.sm"), groups: ALL },
  { name: "botón fantasma ícono sm", app: part("btn-ghost-ic-sm", ".btn"), mockup: "V3-Monitors", ref: ref("button.btn.ghost.ic.sm"), groups: [...ALL, "ancho"] },
  // F-Settings: peligro («Leave desk»).
  { name: "botón peligro", app: part("btn-danger", ".btn"), mockup: "F-Settings", ref: ref("button.btn.danger"), groups: ALL },
  // Segmentados: el de alineación de V3-Arrangement (md) y el de machine de V3-Monitors (lg).
  { name: "segmentado md", app: part("seg-md", ".seg"), mockup: "V3-Arrangement", ref: ref(".seg"), groups: ["relleno", "radio", "borde", "fondo", "sombra"] },
  { name: "segmentado md · indicador", app: part("seg-md", ".ind"), mockup: "V3-Arrangement", ref: ref(".seg .ind"), groups: IND },
  { name: "segmentado md · opción elegida", app: part("seg-md", '[aria-checked="true"]'), mockup: "V3-Arrangement", ref: ref('.seg .sb[aria-checked="true"]'), groups: ALL },
  { name: "segmentado md · opción", app: part("seg-md", '[aria-checked="false"]'), mockup: "V3-Arrangement", ref: ref('.seg .sb[aria-checked="false"]'), groups: ["fuente"] },
  { name: "segmentado lg", app: part("seg-lg", ".seg"), mockup: "V3-Monitors", ref: ref(".seg"), groups: ["relleno", "radio", "borde", "fondo", "sombra"] },
  { name: "segmentado lg · indicador", app: part("seg-lg", ".ind"), mockup: "V3-Monitors", ref: ref(".seg .ind"), groups: IND },
  { name: "segmentado lg · opción elegida", app: part("seg-lg", '[aria-checked="true"]'), mockup: "V3-Monitors", ref: ref('.seg .segb[aria-checked="true"]'), groups: ALL },
  { name: "segmentado lg · opción", app: part("seg-lg", '[aria-checked="false"]'), mockup: "V3-Monitors", ref: ref('.seg .segb[aria-checked="false"]'), groups: ["fuente"] },
  // F-Settings, pestaña «Keyboard and mouse»: interruptor encendido y apagado, con su perilla.
  { name: "interruptor encendido", app: part("sw-on", ".sw"), mockup: "F-Settings", state: "keyboard", ref: ref("button.sw:not(.off)"), groups: SWITCH },
  { name: "interruptor encendido · perilla", app: part("sw-on", ".sw", "::after"), mockup: "F-Settings", state: "keyboard", ref: ref("button.sw:not(.off)", "::after"), groups: KNOB },
  { name: "interruptor apagado", app: part("sw-off", ".sw"), mockup: "F-Settings", state: "keyboard", ref: ref("button.sw.off"), groups: SWITCH },
  { name: "interruptor apagado · perilla", app: part("sw-off", ".sw", "::after"), mockup: "F-Settings", state: "keyboard", ref: ref("button.sw.off", "::after"), groups: KNOB },
  // V3-Desk: el chip contador de Tunnels y las tarjetas (la de escritorios y la de machine).
  { name: "chip contador", app: part("chip-count", ".chip"), mockup: "V3-Desk", ref: ref(".nav .chip"), groups: ALL },
  { name: "tarjeta", app: part("card", ".card"), mockup: "V3-Desk", ref: ref("div.card"), groups: ["relleno", "radio", "borde", "fondo", "sombra", "transition", "animation"] },
  // `.mcard` quita el relleno de abajo para la gráfica: de la tarjeta link se mira el resto.
  { name: "tarjeta link", app: part("card-link", ".card"), mockup: "V3-Desk", ref: ref("a.card.mcard"), groups: ["radio", "borde", "fondo", "sombra", "transition", "animation"] },
];
