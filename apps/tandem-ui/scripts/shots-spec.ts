// Qué captura el arnés (`pnpm shots`): cada pantalla de la app en modo demo, contra qué
// mockup se compara y qué zonas se miden aparte.
//
// Las diferencias permitidas con el mockup (`mockups/README.md`, «Lo que los mockups dicen y
// DECISIONS corrige») se tapan igual en las dos imágenes antes de comparar: `ignore` da el
// selector del elemento en el mockup y la razón.

import { PNG } from "pngjs";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Ignore {
  /** Selector del elemento en el mockup cuya caja se tapa en las dos capturas. */
  mockupSelector: string;
  reason: string;
}

export interface Region {
  name: string;
  /** Caja fija, o la de un selector en el mockup. */
  rect?: Rect;
  mockupSelector?: string;
  /** Umbrales que hacen fallar `--check` (si se dan). */
  min?: { similarity: number; worstTile: number };
}

export interface ShotSpec {
  /** Nombre del archivo: `docs/screenshots/v03/<name>.png`. */
  name: string;
  /** Página de la app (`index.html` o `tray.html`). */
  page: "index.html" | "tray.html";
  /** Parámetros del modo demo, sin `?`. */
  query: string;
  hash: string;
  mockup: string;
  width: number;
  height: number;
  /** Teclas a pulsar antes de capturar (por ejemplo, abrir la paleta). */
  keys?: string[];
  /** Selectores a los que se hace clic antes de capturar (los pasos del estado del mockup). */
  clicks?: string[];
  /** Estado con nombre del mockup (`docs/mockups-png/<mockup>--<state>.png`). */
  state?: string;
  /** Umbral de la pantalla entera (si se da, `--check` lo exige). */
  min?: { similarity: number; worstTile: number };
  ignore?: Ignore[];
  regions?: Region[];
}

/** La barra lateral de las pantallas principales (V3-Puesto, V3-Acomodo, V3-Red…). */
export const SIDEBAR: Region = { name: "barra lateral", mockupSelector: 'nav[aria-label="Principal"]' };

/** D1: no hay tercera máquina, así que la barra lateral no tiene «Agregar máquina». */
export const NO_ADD_MACHINE: Ignore = {
  mockupSelector: 'nav[aria-label="Principal"] a[href="F-AgregarHost.dc.html"]',
  reason: "D1: sin «Agregar máquina» (exactamente dos máquinas)",
};

/**
 * Solo `V3-Maquina` dice «Mi puesto» junto al logo; la barra lateral es una sola para todas las
 * pantallas y su referencia es la de `V3-Puesto` (T-112).
 */
export const DESK_LABEL: Ignore = {
  mockupSelector: 'nav[aria-label="Principal"] > div:first-child > span.mut',
  reason: "la barra lateral es la de V3-Puesto (sin «Mi puesto»)",
};

/** Criterio de T-112: la barra lateral igual a la de `V3-Puesto`. */
export const SIDEBAR_GATE = { similarity: 0.99, worstTile: 0.97 };

const W = 1280;
const H = 860;
const DEMO = "demo&still";

/** Criterio de aceptación de cada pantalla (ACCEPTANCE: similitud y peor cuadro). */
export const SCREEN_GATE = { similarity: 0.99, worstTile: 0.97 };

// T-117: el acomodo inicial, la otra máquina en cada lado × cada alineación (con los mismos
// clics que el mockup), cada pantalla elegida y la prueba de cruce.
const ACOMODO_SIDES = [
  ["arriba", "Arriba"],
  ["abajo", "Abajo"],
  ["izquierda", "Izquierda"],
  ["derecha", "Derecha"],
] as const;
const ACOMODO_ALIGNS = ["inicio", "centro", "fin"] as const;
const acomodoShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/acomodo",
  mockup: "V3-Acomodo",
  width: W,
  height: H,
  ignore: [NO_ADD_MACHINE],
  regions: [SIDEBAR],
  min: SCREEN_GATE,
  ...extra,
});
/** Los botones «Simular…» son solo del mockup (`mockups/README.md`): se tapan. */
const SIMULATE: Ignore[] = [
  { mockupSelector: 'button.sim:has-text("Simular cruce")', reason: "«Simular cruce» es solo del mockup" },
  { mockupSelector: 'button.sim:has-text("no cruza")', reason: "«Simular que no cruza» es solo del mockup" },
];
const cruceShot = (name: string, cross: string, state?: string): ShotSpec => ({
  name,
  page: "index.html",
  query: `${DEMO}${cross ? `&cross=${cross}` : ""}`,
  hash: "#/acomodo/cruce",
  mockup: "F-PruebaCruce",
  state,
  width: W,
  height: H,
  ...(state ? {} : { ignore: SIMULATE }),
  min: SCREEN_GATE,
});
export const ACOMODO: ShotSpec[] = [
  acomodoShot("acomodo"),
  ...ACOMODO_SIDES.flatMap(([side, label]) =>
    ACOMODO_ALIGNS.map((al, i) =>
      acomodoShot(`acomodo-${side}-${al}`, { state: `lado-${side}-${al}`, clicks: [`.slot[aria-label="${label}"]`, `.seg .sb >> nth=${i}`] }),
    ),
  ),
  acomodoShot("acomodo-pantalla-mac", { state: "pantalla-mac", clicks: ['.scr:has-text("MacBook Pro")'] }),
  acomodoShot("acomodo-no-compatible", { state: "pantalla-no-compatible", clicks: ['.scr:has-text("Odyssey G5 32")'] }),
  cruceShot("cruce", ""),
  cruceShot("cruce-cruzo", "ok", "cruzo"),
  cruceShot("cruce-no-cruza", "timeout", "no-cruza"),
  // Borde equivocado: el mockup no lo dibuja (ACCEPTANCE: mismos componentes, sin umbral).
  { ...cruceShot("cruce-otro-borde", "wrong", "no-cruza"), min: undefined },
];

export const SHOTS: ShotSpec[] = [
  {
    name: "puesto",
    page: "index.html",
    query: DEMO,
    hash: "#/puesto",
    mockup: "V3-Puesto",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE, { mockupSelector: 'a[href="F-AgregarHost.dc.html"].card', reason: "D1: sin tarjeta «Agregar máquina»" }],
    regions: [{ ...SIDEBAR, min: SIDEBAR_GATE }],
  },
  // D2 (alcance de T-126): el mismo puesto con dos Windows y con dos Mac. Los nombres y
  // colores cambian, así que no tienen umbral: sirven para ver cada combinación.
  { name: "puesto-win-win", page: "index.html", query: "demo=win-win&still", hash: "#/puesto", mockup: "V3-Puesto", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  { name: "puesto-mac-mac", page: "index.html", query: "demo=mac-mac&still", hash: "#/puesto", mockup: "V3-Puesto", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  { name: "tuneles", page: "index.html", query: DEMO, hash: "#/tuneles", mockup: "V3-Computo", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  { name: "monitores", page: "index.html", query: DEMO, hash: "#/monitores", mockup: "V3-Monitores", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  ...ACOMODO,
  { name: "red", page: "index.html", query: DEMO, hash: "#/red", mockup: "V3-Red", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  // T-115: la página de la otra máquina, con umbral en la barra lateral y en el contenido.
  // `F-MaquinaMac` (esta máquina) no tiene barra lateral (G3 la pide) ni el selector de color
  // (alcance 4 de T-115): se captura para verla, sin umbral.
  {
    name: "maquina-b",
    page: "index.html",
    query: DEMO,
    hash: "#/maquina/b",
    mockup: "V3-Maquina",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE, DESK_LABEL],
    regions: [
      { ...SIDEBAR, min: SIDEBAR_GATE },
      { name: "contenido", mockupSelector: "main", min: SIDEBAR_GATE },
    ],
  },
  { name: "maquina-a", page: "index.html", query: DEMO, hash: "#/maquina/a", mockup: "F-MaquinaMac", width: W, height: H },
  { name: "ajustes", page: "index.html", query: DEMO, hash: "#/ajustes", mockup: "F-Ajustes", width: W, height: H },
  { name: "paleta", page: "index.html", query: DEMO, hash: "#/puesto", mockup: "F-Paleta", width: W, height: H, keys: ["Control+k"] },
  { name: "estados", page: "index.html", query: `${DEMO}&trouble=peer-off`, hash: "#/puesto", mockup: "V3-Estados", width: W, height: H },
  { name: "bandeja", page: "tray.html", query: DEMO, hash: "", mockup: "V3-Bandeja", width: 380, height: 600 },
];

// ---- Imágenes -----------------------------------------------------------------------------

/** Pinta `rects` de un color fijo (las zonas que no se comparan). */
export function mask(png: Buffer, rects: Rect[], rgb: [number, number, number] = [255, 0, 255]): Buffer {
  const img = PNG.sync.read(png);
  for (const r of rects) {
    const x0 = Math.max(0, Math.floor(r.x));
    const y0 = Math.max(0, Math.floor(r.y));
    const x1 = Math.min(img.width, Math.ceil(r.x + r.width));
    const y1 = Math.min(img.height, Math.ceil(r.y + r.height));
    for (let y = y0; y < y1; y++) {
      for (let x = x0; x < x1; x++) {
        const i = (y * img.width + x) * 4;
        img.data[i] = rgb[0];
        img.data[i + 1] = rgb[1];
        img.data[i + 2] = rgb[2];
        img.data[i + 3] = 255;
      }
    }
  }
  return PNG.sync.write(img);
}

/** La URL de la app servida por el arnés. */
export function appUrl(origin: string, s: Pick<ShotSpec, "page" | "query" | "hash">): string {
  return `${origin}/${s.page}${s.query ? `?${s.query}` : ""}${s.hash}`;
}
