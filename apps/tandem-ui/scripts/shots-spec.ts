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
  /** Sacar el mouse de la ventana tras los clics: el mockup redibuja el botón y pierde el hover. */
  unhover?: boolean;
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

// T-120: Ajustes (una captura por pestaña) y los estados difíciles. Los dos mockups no tienen
// barra lateral (G3/D16): se compara el contenido, desde la esquina de su título.
const NONE = "#sin-contraparte-en-la-app";
const ajustesShot = (name: string, state: string | undefined, clicks: string[], masks: ContentMask[] = []): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/ajustes",
  mockup: "F-Ajustes",
  state,
  clicks,
  width: W,
  height: H,
  content: {
    appWidth: MAC_PAGE_WIDTH,
    appStyle: 'nav[aria-label="Principal"] { flex: 0 0 220px !important; }',
    app: "main .head",
    mockup: "h1",
    masks,
    min: SIDEBAR_GATE,
  },
});
const tab = (label: string) => `[role="tab"]:has-text("${label}")`;
export const AJUSTES: ShotSpec[] = [
  ajustesShot("ajustes", undefined, [], [
    { app: ".pane > .row:nth-child(1) > .mut", mockup: ".pane > .row:nth-child(1) > input", reason: "El nombre del puesto es texto: el campo editable del mockup no tiene función aquí (§3.8)" },
    { app: NONE, mockup: 'button.danger:has-text("Sacar del puesto")', reason: "D1: sin «Sacar del puesto» (exactamente dos máquinas)" },
    { app: ".pane > .row:nth-child(3) > button", mockup: ".pane > .row:nth-child(3) > button.ic", reason: "Sin «Sacar del puesto» a su lado, el lápiz de la otra máquina cae en el borde derecho de la fila" },
    { app: ".pane > .row:nth-child(4) > .fill", mockup: ".pane > .row:nth-child(4) > span", reason: "D1: «Cambiar la otra máquina» en lugar de «Agregar una máquina»" },
    { app: ".pane > .row:nth-child(4) > button", mockup: ".pane > .row:nth-child(4) > a.btn", reason: "D1: «Emparejar de nuevo» en lugar de «Mostrar código»" },
  ]),
  { ...ajustesShot("ajustes-atajos", "atajos", [tab("Atajos")]), query: `${DEMO}&solopc` },
  ajustesShot("ajustes-teclado", "teclado", [tab("Teclado y mouse")]),
  ajustesShot("ajustes-actualizaciones", "actualizaciones", [tab("Actualizaciones")]),
  { ...ajustesShot("ajustes-actualizaciones-buscado", "actualizaciones-buscado", [tab("Actualizaciones"), 'button:has-text("Buscar ahora")']), unhover: true },
  ajustesShot("ajustes-avanzado", "avanzado", [tab("Avanzado")], [
    { app: ".pane > .row:nth-child(3) .path", mockup: ".pane > .row:nth-child(3) .mono", reason: "El agente todavía no cuenta las sesiones del servidor MCP" },
  ]),
];

export const ESTADOS: ShotSpec = {
  name: "estados",
  page: "index.html",
  // `?estados` siembra los textos de ejemplo del mockup (nombres, cifras): se compara todo.
  query: `${DEMO}&estados`,
  hash: "#/estados",
  mockup: "V3-Estados",
  width: W,
  height: H,
  content: {
    appWidth: 220 + 2 * 28 + (1280 - 2 * 28),
    appStyle: 'nav[aria-label="Principal"] { flex: 0 0 220px !important; }',
    app: "main .head",
    mockup: "h1",
    masks: [
      { app: NONE, mockup: 'button.btn:has-text("Enviar por Wi-Fi igual")', reason: "El agente no tiene «enviar por Wi-Fi igual»: sin función, sin botón" },
      { app: "article.st:nth-child(1) p", mockup: "article.st:nth-child(1) p", reason: "D1: la frase no dice «Agrega otra máquina» (T-142)" },
      { app: "article.st:nth-child(1) .act button", mockup: "article.st:nth-child(1) a.btn", reason: "D1: «Emparejar la otra máquina» en lugar de «Agregar máquina» (T-142)" },
    ],
    min: SIDEBAR_GATE,
  },
};

/**
 * T-120: la paleta contra `F-Paleta`. Se tapan los renglones con datos del puesto (el mockup
 * trae escenas, túneles y colores de ejemplo) y la página de fondo, que en la app es la real y
 * en el mockup un esqueleto.
 */
const ROW = (text: string) => `section[role="dialog"] .it:has(span:text-is("${text}"))`;
export const PALETA: ShotSpec = {
  name: "paleta",
  page: "index.html",
  query: DEMO,
  hash: "#/puesto",
  mockup: "F-Paleta",
  width: W,
  height: H,
  keys: ["Meta+k"],
  ignore: [
    { mockupSelector: 'section[role="dialog"] .it:has-text("Trabajo") > span:first-child', reason: "Datos reales: el color del punto es el de la máquina que muestra la escena" },
    { mockupSelector: ROW("Aplicar «Solo PC»"), reason: "Datos reales: las escenas del puesto" },
    { mockupSelector: ROW("Agregar máquina…"), reason: "D1: no hay tercera máquina; se cambia la otra" },
    { mockupSelector: ROW("Ir al túnel «ia-local»"), reason: "Datos reales: los túneles del puesto" },
    { mockupSelector: ROW("Ir al túnel «tandem»"), reason: "Datos reales: los túneles del puesto" },
    { mockupSelector: 'section[role="dialog"] .grp:text-is("Ir a")', reason: "Los túneles reales empujan los grupos siguientes" },
    { mockupSelector: ROW("Monitores"), reason: "Los túneles reales empujan los renglones siguientes" },
    { mockupSelector: ROW("Red"), reason: "Los túneles reales empujan los renglones siguientes" },
  ],
  regions: [{ name: "diálogo", mockupSelector: 'section[role="dialog"]', min: SCREEN_GATE }],
  min: SCREEN_GATE,
};

/**
 * T-120: la bandeja contra `V3-Bandeja`. Los datos del puesto reemplazan a los de ejemplo
 * (enlace, carga, escenas y túneles); como los túneles reales empujan el aviso hacia abajo, el
 * aviso también se tapa. «Bloquear aquí» no existe en la app (DECISIONS, T-112).
 */
export const BANDEJA: ShotSpec = {
  name: "bandeja",
  page: "tray.html",
  // `V3-Bandeja` agrupa los túneles en tres proyectos; `tunnels=notas` los siembra así.
  query: `${DEMO}&tunnels=notas`,
  hash: "",
  mockup: "V3-Bandeja",
  width: 380,
  height: 600,
  content: {
    appWidth: 380,
    app: ".pop",
    mockup: "x-dc > div",
    masks: [
      { app: ".ln", mockup: ".mono.mut:has-text(\"USB-C\")", reason: "Datos reales: el enlace y su latencia" },
      { app: ".box:not(.tl) .sm", mockup: ".row > b + .mut", reason: "Datos reales: el estado de cada máquina" },
      { app: ".box:not(.tl) .bar", mockup: ".row .bar", reason: "Datos reales: la carga de cada máquina" },
      { app: ".sc:nth-child(3)", mockup: ".sc:nth-child(3)", reason: "Datos reales: las escenas del puesto" },
      { app: ".box.tl", mockup: 'div[style*="gap:2px"]', reason: "Datos reales: los túneles abiertos (un renglón por proyecto, con los nombres reales de las máquinas)" },
      { app: NONE, mockup: '.lnk:has-text("Bloquear aquí")', reason: "Sin la acción en la IPC no hay botón (T-112, G6)" },
    ],
    min: SCREEN_GATE,
  },
};

/**
 * `F-Avisos`: la pila entera de avisos (`?avisos` siembra los tres del mockup, con sus textos,
 * tonos y el botón del primero) contra la del mockup. Para medir, la página de fondo se oculta y
 * la pila (fija en la ventana) pasa a absoluta: el mockup la apila en el flujo, dentro de un
 * cuadro, y como capa fija el texto cae en otro píxel (ver `mockups/README.md`).
 */
export const AVISOS: ShotSpec = {
  name: "avisos",
  page: "index.html",
  query: `${DEMO}&avisos`,
  hash: "#/puesto",
  mockup: "F-Avisos",
  width: W,
  height: 860,
  content: {
    appWidth: W,
    appStyle: "html, body { background: #0b0c0e !important; } .shell { visibility: hidden !important; } .toasts { position: absolute !important; }",
    app: ".toast:nth-child(1)",
    mockup: ".toast:nth-child(1)",
    masks: [],
    min: SCREEN_GATE,
  },
};

// T-119: el asistente de 6 pasos. El demo lo abre en el paso pedido (`wizard=N`) con los datos
// del mockup (MacBook Pro como esta máquina, PC-Escritorio como la otra).
const asisShot = (name: string, mockup: string, query: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: `${DEMO}&${query}`,
  hash: "#/puesto",
  mockup,
  width: W,
  height: H,
  min: SCREEN_GATE,
  ...extra,
});
/** Los botones «Simular…» son solo del mockup (README): en la app esos pasos ocurren de verdad. */
const SIMULAR_ASIS: Ignore[] = [
  { mockupSelector: 'button.sim:has-text("Simular cruce")', reason: "«Simular cruce» es solo del mockup" },
  { mockupSelector: 'button.sim:has-text("Simular otro borde")', reason: "«Simular otro borde» es solo del mockup" },
];
/**
 * `F-Asis5`: su CSS declara `.cur` para el cursor del dibujo y esa clase también la lleva el paso
 * actual de la lista (`.stl.cur`): el mockup lo saca del flujo (14 × 14, absoluto) y «Listo» sube
 * 44 px. La app dibuja la lista como las demás pantallas; se tapan los dos pasos afectados.
 */
const ASIS5_LIST: Ignore[] = [
  { rect: { x: 10, y: 138, width: 140, height: 60 }, reason: "F-Asis5: el paso actual sale del flujo por la clase .cur duplicada" },
  { rect: { x: 0, y: 258, width: 300, height: 94 }, reason: "F-Asis5: «Listo» sube 44 px en el mockup por la clase .cur duplicada" },
];
/** `F-Asis5` escribe «de la MacBook a la PC»; la app dice los nombres de las máquinas del puesto. */
const ASIS5_LEAD: Ignore = { rect: { x: 350, y: 106, width: 640, height: 50 }, reason: "F-Asis5: el texto nombra a las máquinas del puesto, no «la MacBook» y «la PC»" };
/** Los textos del veredicto nombran a las máquinas del puesto (el mockup dice «la MacBook»). */
const ASIS5_VERDICT: Ignore = { rect: { x: 350, y: 482, width: 440, height: 40 }, reason: "F-Asis5: el veredicto nombra a las máquinas del puesto" };
const ASIS5: Ignore[] = [...ASIS5_LIST, ASIS5_LEAD];
/** `F-Asis3` escribe «Faltan 1» y no ofrece reiniciar el agente; la app concuerda («Falta 1») y lo ofrece (T-090). */
const PERMS_FOOT: Ignore = { rect: { x: 775, y: 770, width: 200, height: 40 }, reason: "F-Asis3: «Faltan 1» y «Reiniciar agente» (solo en la app)" };
const asisColor = (label: string, state: string) => asisShot(`asistente-maquina-${state}`, "F-Asis1", "wizard=0&admin=pending", { state, clicks: [`[role="radio"][aria-label="${label}"]`] });
export const ASISTENTE: ShotSpec[] = [
  asisShot("asistente-maquina", "F-Asis1", "wizard=0&admin=pending"),
  asisColor("Violeta", "violeta"),
  asisColor("Verde azulado", "verde-azulado"),
  asisColor("Ámbar", "ambar"),
  asisShot("asistente-puesto", "V3-Asistente", "wizard=1&peer=none&pair=code", {
    fills: ["4", "8", "2", "9"].map((text, i) => ({ selector: `input[aria-label="Dígito ${i + 1}"]`, text })),
    // El foco salta solo al dígito siguiente; el mockup no tiene foco en ninguno.
    clicks: [".wiz h1"],
  }),
  // T-145: «Crear un puesto nuevo». El panel (nombre del puesto en el mockup, código en la app) se tapa entero.
  asisShot("asistente-puesto-nuevo", "V3-Asistente", "wizard=1&peer=none&pair=code", {
    state: "nuevo",
    clicks: ['[role="radio"]:has-text("Crear un puesto nuevo")'],
    ignore: [{ rect: { x: 350, y: 315, width: 760, height: 205 }, reason: "T-145: el nombre del puesto no existe en el agente (§3.8); la app muestra el código de emparejamiento (T-097)" }],
  }),
  asisShot("asistente-permisos", "F-Asis3", "wizard=2&perms=missing"),
  asisShot("asistente-permisos-uno", "F-Asis3", "wizard=2&perms=missing", { state: "uno-concedido", clicks: ['button:has-text("Abrir ajustes") >> nth=0'], ignore: [PERMS_FOOT] }),
  asisShot("asistente-permisos-todo", "F-Asis3", "wizard=2&perms=missing", {
    state: "todo-concedido",
    clicks: ['button:has-text("Abrir ajustes") >> nth=0', 'button:has-text("Abrir ajustes") >> nth=0'],
  }),
  asisShot("asistente-teclado", "F-Asis4", "wizard=3"),
  asisShot("asistente-acomodo", "F-Asis5", "wizard=4", { ignore: ASIS5 }),
  asisShot("asistente-acomodo-esperando", "F-Asis5", "wizard=4", { state: "esperando", clicks: ['button:has-text("Probar cruce")'], ignore: [...ASIS5, ASIS5_VERDICT, ...SIMULAR_ASIS] }),
  asisShot("asistente-acomodo-cruzo", "F-Asis5", "wizard=4&cross=ok", { state: "cruzo", clicks: ['button:has-text("Probar cruce")'], ignore: ASIS5 }),
  asisShot("asistente-acomodo-otro-borde", "F-Asis5", "wizard=4&cross=wrong", { state: "otro-borde", clicks: ['button:has-text("Probar cruce")'], ignore: ASIS5 }),
  asisShot("asistente-listo", "F-Asis6", "wizard=5&crossed"),
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
  ...ASISTENTE,
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
  ...AJUSTES,
  PALETA,
  ESTADOS,
  AVISOS,
  BANDEJA,
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
