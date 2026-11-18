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
  mockupSelector?: string;
  /** O una caja fija (solo si no hay un elemento que la delimite). */
  rect?: Rect;
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
  /** Campos a llenar antes de los clics (el nombre de la escena). */
  fills?: { selector: string; text: string }[];
  /** Teclas a pulsar al final, después de los clics (abrir la paleta, grabar un atajo). */
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

// T-116: Monitores (con cada entrada elegida), el editor de escenas y el progreso de una escena.
const monitoresShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/monitores",
  mockup: "V3-Monitores",
  width: W,
  height: H,
  ignore: [NO_ADD_MACHINE],
  regions: [SIDEBAR],
  min: SCREEN_GATE,
  ...extra,
});
/** El editor llena lo mismo que el mockup: «Juego», el atajo ⌃⌥4 y lo que muestra el 34". */
const escenaShot = (name: string, pick: string, state?: string): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/monitores/escena",
  mockup: "F-NuevaEscena",
  state,
  width: W,
  height: H,
  fills: [{ selector: 'input:not([type="hidden"])', text: "Juego" }],
  clicks: [`[role="radio"]:has-text("${pick}")`, "button.kbd"],
  keys: ["Control+Alt+Digit4"],
  min: SCREEN_GATE,
});
/** Los botones «Simular…» del mockup (solo del mockup); en «vuelta atrás» bajan con el texto del error. */
const SIMULAR: Ignore = { mockupSelector: "section.card:nth-of-type(1) > div:last-of-type", reason: "Simular…: botones solo del mockup" };
/** «Dejar de esperar»: el agente no tiene cómo cancelar un encendido (UiCommand), así que la app no ofrece la acción. */
const NO_CANCEL: Ignore = { mockupSelector: "section.card:nth-of-type(2) > div:last-of-type", reason: "Dejar de esperar: sin comando de cancelar en la IPC, no se muestra" };
/** El mockup escribe «de la MacBook»; la app dice el nombre real de la máquina (D10). */
const MACHINE_NAME: Ignore = { rect: { x: 236, y: 228, width: 90, height: 22 }, reason: "«de la MacBook»: en la app va el nombre real de la máquina" };
const progresoShot = (name: string, txn: "run" | "ok" | "fail", state?: string): ShotSpec => ({
  name,
  page: "index.html",
  query: `${DEMO}&txn=${txn}`,
  hash: "#/progreso",
  mockup: "F-Transacciones",
  state,
  width: W,
  height: H,
  ignore: [SIMULAR, NO_CANCEL, ...(txn === "fail" ? [] : [MACHINE_NAME])],
  min: SCREEN_GATE,
});
// T-114: Túneles contra `V3-Computo` en cada estado que dibuja (con los mismos clics que el
// mockup). Sin túneles y con 12 procesos el mockup no los dibuja: se capturan con los mismos
// componentes y tokens, sin umbral (ACCEPTANCE, P2).
const tunelesShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/tuneles",
  mockup: "V3-Computo",
  width: W,
  height: H,
  ignore: [NO_ADD_MACHINE],
  regions: [SIDEBAR],
  min: SCREEN_GATE,
  ...extra,
});
export const TUNELES: ShotSpec[] = [
  tunelesShot("tuneles"),
  tunelesShot("tuneles-proceso-dos", { state: "proceso-2", clicks: ['[role="option"] >> nth=1'] }),
  tunelesShot("tuneles-dos-procesos", { state: "tunel-2-procesos", clicks: ['.tun:has-text("T-101")'] }),
  tunelesShot("tuneles-sin-procesos", { state: "tunel-sin-procesos", clicks: ['.tun:has-text("Revisar PR #66")'] }),
  tunelesShot("tuneles-lora", { state: "tunel-lora", clicks: ['.tun:has-text("Entrenar LoRA")'] }),
  tunelesShot("tuneles-cerrado", { state: "tunel-cerrado", clicks: ['.tun:has-text("ComfyUI")'] }),
  tunelesShot("tuneles-proyecto-plegado", { state: "proyecto-plegado", clicks: ['.proj:has-text("tandem")'] }),
  tunelesShot("tuneles-vacio", { query: `${DEMO}&tunnels=none`, min: undefined }),
  tunelesShot("tuneles-doce-procesos", { query: `${DEMO}&tunnels=many`, min: undefined }),
];

export const MONITORES: ShotSpec[] = [
  monitoresShot("monitores"),
  monitoresShot("monitores-entrada-mac", { state: "entrada-mac", clicks: ['[role="radio"]:has-text("MacBook Pro")'] }),
  escenaShot("escena-nueva", "PC-Escritorio"),
  escenaShot("escena-muestra-mac", "MacBook Pro", "muestra-mac"),
  escenaShot("escena-sin-cambiar", "Sin cambiar", "sin-cambiar"),
  progresoShot("progreso-en-curso", "run"),
  progresoShot("progreso-exito", "ok", "exito"),
  progresoShot("progreso-vuelta-atras", "fail", "vuelta-atras"),
];

/** El hueco vacío que deja D1 en la fila de las tarjetas de máquina de `V3-Puesto`. */
const NO_ADD_CARD: Ignore = { mockupSelector: 'a[href="F-AgregarHost.dc.html"].card', reason: "D1: sin tarjeta «Agregar máquina»" };

/** D2: los escritorios a escala toman el color de cada máquina; con dos del mismo sistema el fondo no puede coincidir con el mockup. */
const D2_DESKS: Ignore = { mockupSelector: 'div:has(> div[role="img"][aria-label^="A escala"])', reason: "D2: el color de las máquinas cambia el fondo de los escritorios" };

export const SHOTS: ShotSpec[] = [
  {
    name: "puesto",
    page: "index.html",
    query: DEMO,
    hash: "#/puesto",
    mockup: "V3-Puesto",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE, NO_ADD_CARD],
    regions: [{ ...SIDEBAR, min: SIDEBAR_GATE }],
    // T-113: toda la pantalla. Solo se tapa el hueco vacío que D1 quita («Agregar máquina»); la
    // app no tiene ninguna tarjeta oculta (Encender/Apagar van dentro de la tarjeta de la otra máquina).
    min: SCREEN_GATE,
  },
  // D2 (alcance de T-126): el mismo puesto con dos Windows y con dos Mac. Los nombres y
  // colores cambian, así que no tienen umbral: sirven para ver cada combinación.
  { name: "puesto-win-win", page: "index.html", query: "demo=win-win&still", hash: "#/puesto", mockup: "V3-Puesto", width: W, height: H, ignore: [NO_ADD_MACHINE, NO_ADD_CARD, D2_DESKS], regions: [SIDEBAR] },
  { name: "puesto-mac-mac", page: "index.html", query: "demo=mac-mac&still", hash: "#/puesto", mockup: "V3-Puesto", width: W, height: H, ignore: [NO_ADD_MACHINE, NO_ADD_CARD, D2_DESKS], regions: [SIDEBAR] },
  ...TUNELES,
  ...MONITORES,
  ...ACOMODO,
  // T-118: la pantalla Red entera contra `V3-Red`. Se tapa lo que D1 quita («Agregar máquina») y
  // el rótulo de la ventana de la gráfica: el agente guarda 60 s, el mockup dice «10 min».
  {
    name: "red",
    page: "index.html",
    query: DEMO,
    hash: "#/red",
    mockup: "V3-Red",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE, { mockupSelector: 'span.mut:has-text("Latencia · 10 min")', reason: "T-102: la gráfica es de 60 s (DESIGN §6), no de 10 min" }],
    regions: [SIDEBAR],
    min: SCREEN_GATE,
  },
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
  { name: "paleta", page: "index.html", query: DEMO, hash: "#/puesto", mockup: "F-Paleta", width: W, height: H, keys: ["Meta+k"] },
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
