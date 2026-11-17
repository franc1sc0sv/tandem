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
  /** Comparar solo el contenido, cuando el mockup no tiene la barra lateral (ver `Content`). */
  content?: Content;
}

/** Una zona tapada en las dos capturas: la caja de `app` en la app y la de `mockup` en el mockup. */
export interface ContentMask {
  app: string;
  mockup?: string;
  /** La diferencia permitida de `mockups/README.md` que la justifica. */
  reason: string;
}

/**
 * Comparación del contenido para un mockup de flujo sin barra lateral (`F-MaquinaMac`): la app
 * se captura con `appWidth` de ancho, para que su contenido mida lo mismo que el del mockup, y
 * se recortan las dos desde la esquina de `app`/`mockup` hasta el borde de abajo.
 */
export interface Content {
  appWidth: number;
  /**
   * CSS que se agrega a la app solo para esta captura. Con la barra lateral flexible, su ancho
   * (y el borde izquierdo del contenido) cae en una fracción de píxel y todo el texto se ve
   * corrido medio píxel; se fija en 220 px, fuera de la zona que se compara.
   */
  appStyle?: string;
  app: string;
  mockup: string;
  masks: ContentMask[];
  min: { similarity: number; worstTile: number };
}

/** La barra lateral de las pantallas principales (V3-Puesto, V3-Acomodo, V3-Red…). */
export const SIDEBAR: Region = { name: "barra lateral", mockupSelector: 'nav[aria-label="Principal"]' };

/** D1: no hay tercera máquina, así que la barra lateral no tiene «Agregar máquina». */
export const NO_ADD_MACHINE: Ignore = {
  mockupSelector: 'nav[aria-label="Principal"] a[href="F-AgregarHost.dc.html"]',
  reason: "D1: sin «Agregar máquina» (exactamente dos máquinas)",
};

/** Criterio de T-112: la barra lateral igual a la de `V3-Puesto`. */
export const SIDEBAR_GATE = { similarity: 0.99, worstTile: 0.97 };

const W = 1280;
/**
 * Ancho de la ventana con el que el contenido de la app (barra lateral fija en 220 px y el
 * relleno de `main`, 2 × 28) mide lo mismo que el de `F-MaquinaMac` (1280 − 2 × 32 = 1216 px).
 */
export const MAC_PAGE_WIDTH = 220 + 2 * 28 + 1216;
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
  {
    name: "maquina-b",
    page: "index.html",
    query: DEMO,
    hash: "#/maquina/b",
    mockup: "V3-Maquina",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE],
    regions: [
      { ...SIDEBAR, min: SIDEBAR_GATE },
      { name: "contenido", mockupSelector: "main", min: SIDEBAR_GATE },
    ],
  },
  // T-115: esta máquina contra `F-MaquinaMac`, en el estado que dibuja (falta un permiso). Solo
  // el contenido, con las diferencias de `mockups/README.md` tapadas.
  {
    name: "maquina-a",
    page: "index.html",
    query: `${DEMO}&trouble=permissions`,
    hash: "#/maquina/a",
    mockup: "F-MaquinaMac",
    width: W,
    height: H,
    content: {
      appWidth: MAC_PAGE_WIDTH,
      appStyle: 'nav[aria-label="Principal"] { flex: 0 0 220px !important; }',
      app: "main header",
      mockup: "header",
      masks: [
        { app: '[role="radiogroup"][aria-label="Color de la máquina"]', reason: "D17: el color de la máquina se edita en su página" },
        { app: ".mod > :last-child", mockup: ".mod > :last-child", reason: "G4: el estado del módulo es el chip uniforme (interruptor)" },
      ],
      min: SIDEBAR_GATE,
    },
  },
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

/** Recorta `rect` de la imagen (acotado a sus bordes). */
export function crop(png: Buffer, rect: Rect): Buffer {
  const img = PNG.sync.read(png);
  const x0 = Math.max(0, Math.floor(rect.x));
  const y0 = Math.max(0, Math.floor(rect.y));
  const w = Math.max(0, Math.min(img.width - x0, Math.floor(rect.width)));
  const h = Math.max(0, Math.min(img.height - y0, Math.floor(rect.height)));
  const out = new PNG({ width: w, height: h });
  for (let y = 0; y < h; y++) img.data.copy(out.data, y * w * 4, ((y0 + y) * img.width + x0) * 4, ((y0 + y) * img.width + x0 + w) * 4);
  return PNG.sync.write(out);
}

/** La URL de la app servida por el arnés. */
export function appUrl(origin: string, s: Pick<ShotSpec, "page" | "query" | "hash">): string {
  return `${origin}/${s.page}${s.query ? `?${s.query}` : ""}${s.hash}`;
}
