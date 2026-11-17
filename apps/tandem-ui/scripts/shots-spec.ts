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

/** Criterio de T-112: la barra lateral igual a la de `V3-Puesto`. */
export const SIDEBAR_GATE = { similarity: 0.99, worstTile: 0.97 };

const W = 1280;
const H = 860;
const DEMO = "demo&still";

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
  { name: "tuneles", page: "index.html", query: DEMO, hash: "#/tuneles", mockup: "V3-Computo", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  { name: "monitores", page: "index.html", query: DEMO, hash: "#/monitores", mockup: "V3-Monitores", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  { name: "acomodo", page: "index.html", query: DEMO, hash: "#/acomodo", mockup: "V3-Acomodo", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  { name: "red", page: "index.html", query: DEMO, hash: "#/red", mockup: "V3-Red", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  { name: "maquina-b", page: "index.html", query: DEMO, hash: "#/maquina/b", mockup: "V3-Maquina", width: W, height: H, ignore: [NO_ADD_MACHINE], regions: [SIDEBAR] },
  { name: "maquina-a", page: "index.html", query: DEMO, hash: "#/maquina/a", mockup: "F-MaquinaMac", width: W, height: H },
  { name: "ajustes", page: "index.html", query: DEMO, hash: "#/ajustes", mockup: "F-Ajustes", width: W, height: H },
  { name: "paleta", page: "index.html", query: DEMO, hash: "#/puesto", mockup: "F-Paleta", width: W, height: H, keys: ["Control+k"] },
  { name: "estados", page: "index.html", query: `${DEMO}&trouble=peer-off`, hash: "#/puesto", mockup: "V3-Estados", width: W, height: H },
  { name: "bandeja", page: "tray.html", query: DEMO, hash: "", mockup: "V3-Bandeja", width: 380, height: 600 },
];

// ---- Imágenes -----------------------------------------------------------------------------

/** Recorta `rect` (ajustado a la imagen) y lo devuelve como PNG. */
export function crop(png: Buffer, rect: Rect): Buffer {
  const src = PNG.sync.read(png);
  const x = Math.max(0, Math.floor(rect.x));
  const y = Math.max(0, Math.floor(rect.y));
  const w = Math.max(1, Math.min(src.width - x, Math.ceil(rect.width)));
  const h = Math.max(1, Math.min(src.height - y, Math.ceil(rect.height)));
  const out = new PNG({ width: w, height: h });
  PNG.bitblt(src, out, x, y, w, h, 0, 0);
  return PNG.sync.write(out);
}

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
