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
 * Comparación del contenido para un mockup de flujo sin barra lateral (`F-MachineMac`): la app
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

/** La barra lateral de las pantallas principales (V3-Desk, V3-Arrangement, V3-Network…). */
export const SIDEBAR: Region = { name: "sidebar", mockupSelector: 'nav[aria-label="Main"]' };

/** D1: no hay tercera máquina, así que la barra lateral no tiene «Agregar máquina». */
export const NO_ADD_MACHINE: Ignore = {
  mockupSelector: 'nav[aria-label="Main"] a[href="F-AddHost.dc.html"]',
  reason: "D1: sin «Agregar máquina» (exactamente dos máquinas)",
};

/** Criterio de T-112: la barra lateral igual a la de `V3-Desk`. */
export const SIDEBAR_GATE = { similarity: 0.99, worstTile: 0.97 };

const W = 1280;
/**
 * Ancho de la ventana con el que el contenido de la app (barra lateral fija en 220 px y el
 * relleno de `main`, 2 × 28) mide lo mismo que el de `F-MachineMac` (1280 − 2 × 32 = 1216 px).
 */
export const MAC_PAGE_WIDTH = 220 + 2 * 28 + 1216;
const H = 860;
const DEMO = "demo&still";

/** Criterio de aceptación de cada pantalla (ACCEPTANCE: similitud y peor cuadro). */
export const SCREEN_GATE = { similarity: 0.99, worstTile: 0.97 };

// T-117: el acomodo inicial, the other machine en cada lado × cada alineación (con los mismos
// clics que el mockup), cada pantalla elegida y la prueba de cruce.
const ACOMODO_SIDES = [
  ["top", "Top"],
  ["bottom", "Bottom"],
  ["left", "Left"],
  ["right", "Right"],
] as const;
const ACOMODO_ALIGNS = ["start", "center", "end"] as const;
const acomodoShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/acomodo",
  mockup: "V3-Arrangement",
  width: W,
  height: H,
  ignore: [NO_ADD_MACHINE],
  regions: [SIDEBAR],
  min: SCREEN_GATE,
  ...extra,
});
/** Los botones «Simular…» son solo del mockup (`mockups/README.md`): se tapan. */
const SIMULATE: Ignore[] = [
  { mockupSelector: 'button.sim:has-text("Simulate crossing")', reason: "«Simular cruce» es solo del mockup" },
  { mockupSelector: 'button.sim:has-text("Simulate no crossing")', reason: "«Simular que no cruza» es solo del mockup" },
];
const cruceShot = (name: string, cross: string, state?: string): ShotSpec => ({
  name,
  page: "index.html",
  query: `${DEMO}${cross ? `&cross=${cross}` : ""}`,
  hash: "#/acomodo/cruce",
  mockup: "F-CrossingTest",
  state,
  width: W,
  height: H,
  ...(state ? {} : { ignore: SIMULATE }),
  min: SCREEN_GATE,
});
export const ACOMODO: ShotSpec[] = [
  acomodoShot("arrangement"),
  ...ACOMODO_SIDES.flatMap(([side, label]) =>
    ACOMODO_ALIGNS.map((al, i) =>
      acomodoShot(`arrangement-${side}-${al}`, { state: `side-${side}-${al}`, clicks: [`.slot[aria-label="${label}"]`, `.seg .sb >> nth=${i}`] }),
    ),
  ),
  acomodoShot("arrangement-screen-mac", { state: "screen-mac", clicks: ['.scr:has-text("MacBook Pro")'] }),
  acomodoShot("arrangement-unsupported", { state: "screen-unsupported", clicks: ['.scr:has-text("Odyssey G5 32")'] }),
  cruceShot("crossing", ""),
  cruceShot("crossing-crossed", "ok", "crossed"),
  cruceShot("crossing-no-cross", "timeout", "no-cross"),
  // Borde equivocado: el mockup no lo dibuja (ACCEPTANCE: mismos componentes, sin umbral).
  { ...cruceShot("crossing-other-edge", "wrong", "no-cross"), min: undefined },
];

// T-116: Monitores (con cada entrada elegida), el editor de escenas y el progreso de una escena.
const monitoresShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/monitores",
  mockup: "V3-Monitors",
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
  mockup: "F-NewScene",
  state,
  width: W,
  height: H,
  fills: [{ selector: 'input:not([type="hidden"])', text: "Game" }],
  clicks: [`[role="radio"]:has-text("${pick}")`, "button.kbd"],
  keys: ["Control+Alt+Digit4"],
  min: SCREEN_GATE,
});
/** Los botones «Simular…» del mockup (solo del mockup); en «vuelta atrás» bajan con el texto del error. */
const SIMULAR: Ignore = { mockupSelector: "section.card:nth-of-type(1) > div:last-of-type", reason: "Simular…: botones solo del mockup" };
/** «Dejar de esperar»: el agente no tiene cómo cancelar un encendido (UiCommand), así que la app no ofrece la acción. */
const NO_CANCEL: Ignore = { mockupSelector: "section.card:nth-of-type(2) > div:last-of-type", reason: "Dejar de esperar: sin comando de cancelar en la IPC, no se muestra" };
/** El mockup escribe «de the MacBook»; la app dice el nombre real de la máquina (D10). */
const MACHINE_NAME: Ignore = { rect: { x: 236, y: 228, width: 90, height: 22 }, reason: "«de the MacBook»: en la app va el nombre real de la máquina" };
const progresoShot = (name: string, txn: "run" | "ok" | "fail", state?: string): ShotSpec => ({
  name,
  page: "index.html",
  query: `${DEMO}&txn=${txn}`,
  hash: "#/progreso",
  mockup: "F-Transactions",
  state,
  width: W,
  height: H,
  ignore: [SIMULAR, NO_CANCEL, ...(txn === "fail" ? [] : [MACHINE_NAME])],
  min: SCREEN_GATE,
});
// T-114: Túneles contra `V3-Compute` en cada estado que dibuja (con los mismos clics que el
// mockup). Sin túneles y con 12 procesos el mockup no los dibuja: se capturan con los mismos
// componentes y tokens, sin umbral (ACCEPTANCE, P2).
const tunelesShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/tuneles",
  mockup: "V3-Compute",
  width: W,
  height: H,
  ignore: [NO_ADD_MACHINE],
  regions: [SIDEBAR],
  min: SCREEN_GATE,
  ...extra,
});
export const TUNELES: ShotSpec[] = [
  tunelesShot("tunnels"),
  tunelesShot("tunnels-process-two", { state: "process-2", clicks: ['[role="option"] >> nth=1'] }),
  tunelesShot("tunnels-two-processes", { state: "tunnel-2-processes", clicks: ['.tun:has-text("T-101")'] }),
  tunelesShot("tunnels-no-processes", { state: "tunnel-no-processes", clicks: ['.tun:has-text("Review PR #66")'] }),
  tunelesShot("tunnels-lora", { state: "tunnel-lora", clicks: ['.tun:has-text("Train style LoRA")'] }),
  tunelesShot("tunnels-closed", { state: "tunnel-closed", clicks: ['.tun:has-text("ComfyUI")'] }),
  tunelesShot("tunnels-project-collapsed", { state: "project-collapsed", clicks: ['.proj:has-text("tandem")'] }),
  tunelesShot("tunnels-empty", { query: `${DEMO}&tunnels=none`, min: undefined }),
  tunelesShot("tunnels-twelve-processes", { query: `${DEMO}&tunnels=many`, min: undefined }),
];

export const MONITORES: ShotSpec[] = [
  monitoresShot("monitors"),
  monitoresShot("monitors-input-mac", { state: "input-mac", clicks: ['[role="radio"]:has-text("MacBook Pro")'] }),
  escenaShot("scene-new", "PC-Desktop"),
  escenaShot("scene-shows-mac", "MacBook Pro", "shows-mac"),
  escenaShot("scene-unchanged", "Keep as is", "unchanged"),
  progresoShot("progress-in-progress", "run"),
  progresoShot("progress-success", "ok", "success"),
  progresoShot("progress-rollback", "fail", "rollback"),
];

/** El hueco vacío que deja D1 en la fila de las tarjetas de máquina de `V3-Desk`. */
const NO_ADD_CARD: Ignore = { mockupSelector: 'a[href="F-AddHost.dc.html"].card', reason: "D1: sin tarjeta «Agregar máquina»" };

/** D2: los escritorios a escala toman el color de cada máquina; con dos del mismo sistema el fondo no puede coincidir con el mockup. */
const D2_DESKS: Ignore = { mockupSelector: 'div:has(> div[role="img"][aria-label^="To scale"])', reason: "D2: el color de las máquinas cambia el fondo de los escritorios" };

// T-120: Ajustes (una captura por pestaña) y los estados difíciles. Los dos mockups no tienen
// barra lateral (G3/D16): se compara el contenido, desde la esquina de su título.
const NONE = "#sin-contraparte-en-la-app";
const ajustesShot = (name: string, state: string | undefined, clicks: string[], masks: ContentMask[] = []): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/ajustes",
  mockup: "F-Settings",
  state,
  clicks,
  width: W,
  height: H,
  content: {
    appWidth: MAC_PAGE_WIDTH,
    appStyle: 'nav[aria-label="Main"] { flex: 0 0 220px !important; }',
    app: "main .head",
    mockup: "h1",
    masks,
    min: SIDEBAR_GATE,
  },
});
const tab = (label: string) => `[role="tab"]:has-text("${label}")`;
export const AJUSTES: ShotSpec[] = [
  ajustesShot("settings", undefined, [], [
    { app: ".pane > .row:nth-child(1) > .mut", mockup: ".pane > .row:nth-child(1) > input", reason: "El nombre del puesto es texto: el campo editable del mockup no tiene función aquí (§3.8)" },
    { app: NONE, mockup: 'button.danger:has-text("Remove from desk")', reason: "D1: sin «Sacar del puesto» (exactamente dos máquinas)" },
    { app: ".pane > .row:nth-child(3) > button", mockup: ".pane > .row:nth-child(3) > button.ic", reason: "Sin «Sacar del puesto» a su lado, el lápiz de the other machine cae en el borde derecho de la fila" },
    { app: ".pane > .row:nth-child(4) > .fill", mockup: ".pane > .row:nth-child(4) > span", reason: "D1: «Change the other machine» en lugar de «Agregar una máquina»" },
    { app: ".pane > .row:nth-child(4) > button", mockup: ".pane > .row:nth-child(4) > a.btn", reason: "D1: «Pair again» en lugar de «Mostrar código»" },
  ]),
  { ...ajustesShot("settings-shortcuts", "shortcuts", [tab("Shortcuts")]), query: `${DEMO}&solopc` },
  ajustesShot("settings-keyboard", "keyboard", [tab("Keyboard and mouse")]),
  ajustesShot("settings-updates", "updates", [tab("Updates")]),
  { ...ajustesShot("settings-updates-checked", "updates-checked", [tab("Updates"), 'button:has-text("Check now")']), unhover: true },
  ajustesShot("settings-advanced", "advanced", [tab("Advanced")], [
    { app: ".pane > .row:nth-child(3) .path", mockup: ".pane > .row:nth-child(3) .mono", reason: "El agente todavía no cuenta las sesiones del servidor MCP" },
  ]),
];

export const ESTADOS: ShotSpec = {
  name: "states",
  page: "index.html",
  // `?estados` siembra los textos de ejemplo del mockup (nombres, cifras): se compara todo.
  query: `${DEMO}&estados`,
  hash: "#/estados",
  mockup: "V3-States",
  width: W,
  height: H,
  content: {
    appWidth: 220 + 2 * 28 + (1280 - 2 * 28),
    appStyle: 'nav[aria-label="Main"] { flex: 0 0 220px !important; }',
    app: "main .head",
    mockup: "h1",
    masks: [
      { app: NONE, mockup: 'button.btn:has-text("Send over Wi-Fi anyway")', reason: "El agente no tiene «enviar por Wi-Fi igual»: sin función, sin botón" },
      { app: "article.st:nth-child(1) p", mockup: "article.st:nth-child(1) p", reason: "D1: la frase no dice «Agrega otra máquina» (T-142)" },
      { app: "article.st:nth-child(1) .act button", mockup: "article.st:nth-child(1) a.btn", reason: "D1: «Pair the other machine» en lugar de «Agregar máquina» (T-142)" },
    ],
    min: SIDEBAR_GATE,
  },
};

/**
 * T-120: la paleta contra `F-Palette`. Se tapan los renglones con datos del puesto (el mockup
 * trae escenas, túneles y colores de ejemplo) y la página de fondo, que en la app es la real y
 * en el mockup un esqueleto.
 */
const ROW = (text: string) => `section[role="dialog"] .it:has(span:text-is("${text}"))`;
export const PALETA: ShotSpec = {
  name: "palette",
  page: "index.html",
  query: `${DEMO}&paleta`,
  hash: "#/puesto",
  mockup: "F-Palette",
  width: W,
  height: H,
  keys: ["Meta+k"],
  ignore: [
    { mockupSelector: ROW("Add machine…"), reason: "D1: no hay tercera máquina; se cambia la otra" },
  ],
  regions: [{ name: "dialog", mockupSelector: 'section[role="dialog"]', min: SCREEN_GATE }],
  min: SCREEN_GATE,
};

/** T-144: los estados de `F-Palette` que escribe en el campo (tecla a tecla, como el mockup, que filtra al teclear). */
const PALETA_ESCRIBE = (state: string, text: string): ShotSpec => ({
  ...PALETA,
  name: `palette-${state}`,
  state,
  keys: ["Meta+k", ...text.split("")],
  regions: [{ name: "dialog", mockupSelector: 'section[role="dialog"]', min: SCREEN_GATE }],
});
export const PALETA_ESTADOS: ShotSpec[] = [PALETA_ESCRIBE("filter", "only"), PALETA_ESCRIBE("no-results", "zzz")];

/**
 * T-120: la bandeja contra `V3-Tray`. Los datos del puesto reemplazan a los de ejemplo
 * (enlace, carga, escenas y túneles); como los túneles reales empujan el aviso hacia abajo, el
 * aviso también se tapa. «Lock here» no existe en la app (DECISIONS, T-112).
 */
export const BANDEJA: ShotSpec = {
  name: "tray",
  page: "tray.html",
  // `bandeja` siembra los datos del mockup: 0.4 ms, la Mac al 18 %, «PC only» y tres proyectos.
  query: `${DEMO}&bandeja`,
  hash: "",
  mockup: "V3-Tray",
  width: 380,
  height: 600,
  content: {
    appWidth: 380,
    app: ".pop",
    mockup: "x-dc > div",
    masks: [
      { app: NONE, mockup: '.lnk:has-text("Lock here")', reason: "Sin la acción en la IPC no hay botón (T-112, G6)" },
    ],
    min: SCREEN_GATE,
  },
};

/**
 * `F-Notices`: la pila entera de avisos (`?avisos` siembra los tres del mockup, con sus textos,
 * tonos y el botón del primero) contra la del mockup. Para medir, la página de fondo se oculta y
 * la pila (fija en la ventana) pasa a absoluta: el mockup la apila en el flujo, dentro de un
 * cuadro, y como capa fija el texto cae en otro píxel (ver `mockups/README.md`).
 */
export const AVISOS: ShotSpec = {
  name: "notices",
  page: "index.html",
  query: `${DEMO}&avisos`,
  hash: "#/puesto",
  mockup: "F-Notices",
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
// del mockup (MacBook Pro como this machine, PC-Escritorio como la otra).
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
  { mockupSelector: 'button.sim:has-text("Simulate crossing")', reason: "«Simular cruce» es solo del mockup" },
  { mockupSelector: 'button.sim:has-text("Simulate another edge")', reason: "«Simular otro borde» es solo del mockup" },
];
/**
 * `F-Setup5`: su CSS declara `.cur` para el cursor del dibujo y esa clase también la lleva el paso
 * actual de la lista (`.stl.cur`): el mockup lo saca del flujo (14 × 14, absoluto) y «Listo» sube
 * 44 px. La app dibuja la lista como las demás pantallas; se tapan los dos pasos afectados.
 */
const ASIS5_LIST: Ignore[] = [
  { rect: { x: 10, y: 138, width: 140, height: 60 }, reason: "F-Setup5: el paso actual sale del flujo por la clase .cur duplicada" },
  { rect: { x: 0, y: 258, width: 300, height: 94 }, reason: "F-Setup5: «Listo» sube 44 px en el mockup por la clase .cur duplicada" },
];
/**
 * `F-Setup5` escribe «de the MacBook a la PC»; la app dice los nombres de las máquinas. Solo se tapa
 * lo que sigue a «el mouse de» en el segundo renglón: el primero y el comienzo del segundo son iguales.
 */
const ASIS5_LEAD: Ignore = { rect: { x: 438, y: 134, width: 552, height: 20 }, reason: "F-Setup5: los nombres de las máquinas del puesto («the MacBook», «la PC» en el mockup)" };
/** Los textos del veredicto nombran a las máquinas del puesto (el mockup dice «the MacBook»). */
const ASIS5_VERDICT: Ignore = { rect: { x: 350, y: 482, width: 440, height: 40 }, reason: "F-Setup5: el veredicto nombra a las máquinas del puesto" };
const ASIS5: Ignore[] = [...ASIS5_LIST, ASIS5_LEAD];
/** `F-Setup3` escribe «Faltan 1» y no ofrece reiniciar el agente; la app concuerda («1 missing») y lo ofrece (T-090). */
const PERMS_FOOT: Ignore = { rect: { x: 775, y: 770, width: 200, height: 40 }, reason: "F-Setup3: «Faltan 1» y «Restart agent» (solo en la app)" };
const asisColor = (label: string, state: string) => asisShot(`setup-machine-${state}`, "F-Setup1", "wizard=0&admin=pending", { state, clicks: [`[role="radio"][aria-label="${label}"]`] });
export const ASISTENTE: ShotSpec[] = [
  asisShot("setup-machine", "F-Setup1", "wizard=0&admin=pending"),
  asisColor("Violet", "violet"),
  asisColor("Teal", "teal"),
  asisColor("Amber", "amber"),
  asisShot("setup-desk", "V3-Setup", "wizard=1&peer=none&pair=code", {
    fills: ["4", "8", "2", "9"].map((text, i) => ({ selector: `input[aria-label="Digit ${i + 1}"]`, text })),
    // El foco salta solo al dígito siguiente; el mockup no tiene foco en ninguno.
    clicks: [".wiz h1"],
  }),
  // T-145: «Create a new desk». El panel (nombre del puesto en el mockup, código en la app) se tapa entero.
  asisShot("setup-desk-new", "V3-Setup", "wizard=1&peer=none&pair=code", {
    state: "new",
    clicks: ['[role="radio"]:has-text("Create a new desk")'],
    ignore: [{ rect: { x: 350, y: 315, width: 760, height: 205 }, reason: "T-145: el nombre del puesto no existe en el agente (§3.8); la app muestra el código de emparejamiento (T-097)" }],
  }),
  asisShot("setup-permissions", "F-Setup3", "wizard=2&perms=missing"),
  asisShot("setup-permissions-one", "F-Setup3", "wizard=2&perms=missing", { state: "one-granted", clicks: ['button:has-text("Open settings") >> nth=0'], ignore: [PERMS_FOOT] }),
  asisShot("setup-permissions-all", "F-Setup3", "wizard=2&perms=missing", {
    state: "all-granted",
    clicks: ['button:has-text("Open settings") >> nth=0', 'button:has-text("Open settings") >> nth=0'],
  }),
  asisShot("setup-keyboard", "F-Setup4", "wizard=3"),
  asisShot("setup-arrangement", "F-Setup5", "wizard=4", { ignore: ASIS5 }),
  asisShot("setup-arrangement-waiting", "F-Setup5", "wizard=4", { state: "waiting", clicks: ['button:has-text("Test crossing")'], ignore: [...ASIS5, ASIS5_VERDICT, ...SIMULAR_ASIS] }),
  asisShot("setup-arrangement-crossed", "F-Setup5", "wizard=4&cross=ok", { state: "crossed", clicks: ['button:has-text("Test crossing")'], ignore: ASIS5 }),
  asisShot("setup-arrangement-other-edge", "F-Setup5", "wizard=4&cross=wrong", { state: "other-edge", clicks: ['button:has-text("Test crossing")'], ignore: ASIS5 }),
  asisShot("setup-done", "F-Setup6", "wizard=5&crossed"),
];

export const SHOTS: ShotSpec[] = [
  {
    name: "desk",
    page: "index.html",
    query: DEMO,
    hash: "#/puesto",
    mockup: "V3-Desk",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE, NO_ADD_CARD],
    regions: [{ ...SIDEBAR, min: SIDEBAR_GATE }],
    // T-113: toda la pantalla. Solo se tapa el hueco vacío que D1 quita («Agregar máquina»); la
    // app no tiene ninguna tarjeta oculta (Encender/Apagar van dentro de la tarjeta de the other machine).
    min: SCREEN_GATE,
  },
  // D2 (alcance de T-126): el mismo puesto con dos Windows y con dos Mac. Los nombres y
  // colores cambian, así que no tienen umbral: sirven para ver cada combinación.
  // T-144: aplicar una escena desde el Puesto (los mismos clics que `V3-Desk--escena-*`).
  ...[
    ["scene-macbook-only", "MacBook only"],
    ["scene-game", "Game"],
  ].map(([state, label]): ShotSpec => ({ name: `desk-${state}`, page: "index.html", query: DEMO, hash: "#/puesto", mockup: "V3-Desk", state, width: W, height: H, clicks: [`.scene:has-text("${label}")`], ignore: [NO_ADD_MACHINE, NO_ADD_CARD], min: SCREEN_GATE })),
  { name: "desk-win-win", page: "index.html", query: "demo=win-win&still", hash: "#/puesto", mockup: "V3-Desk", width: W, height: H, ignore: [NO_ADD_MACHINE, NO_ADD_CARD, D2_DESKS], regions: [SIDEBAR] },
  { name: "desk-mac-mac", page: "index.html", query: "demo=mac-mac&still", hash: "#/puesto", mockup: "V3-Desk", width: W, height: H, ignore: [NO_ADD_MACHINE, NO_ADD_CARD, D2_DESKS], regions: [SIDEBAR] },
  ...TUNELES,
  ...MONITORES,
  ...ACOMODO,
  ...ASISTENTE,
  // T-118: la pantalla Red entera contra `V3-Network`. Se tapa lo que D1 quita («Agregar máquina») y
  // el rótulo de la ventana de la gráfica: el agente guarda 60 s, el mockup dice «10 min».
  {
    name: "network",
    page: "index.html",
    query: DEMO,
    hash: "#/red",
    mockup: "V3-Network",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE, { mockupSelector: 'span.mut:has-text("Latency · 10 min")', reason: "T-102: la gráfica es de 60 s (DESIGN §6), no de 10 min" }],
    regions: [SIDEBAR],
    min: SCREEN_GATE,
  },
  // T-115: la página de the other machine, con umbral en la barra lateral y en el contenido.
  {
    name: "machine-b",
    page: "index.html",
    query: DEMO,
    hash: "#/maquina/b",
    mockup: "V3-Machine",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE],
    regions: [
      { ...SIDEBAR, min: SIDEBAR_GATE },
      { name: "content", mockupSelector: "main", min: SIDEBAR_GATE },
    ],
  },
  // T-115: this machine contra `F-MachineMac`, en el estado que dibuja (falta un permiso). Solo
  // el contenido, con las diferencias de `mockups/README.md` tapadas.
  {
    name: "machine-a",
    page: "index.html",
    query: `${DEMO}&trouble=permissions`,
    hash: "#/maquina/a",
    mockup: "F-MachineMac",
    width: W,
    height: H,
    content: {
      appWidth: MAC_PAGE_WIDTH,
      appStyle: 'nav[aria-label="Main"] { flex: 0 0 220px !important; }',
      app: "main header",
      mockup: "header",
      masks: [
        { app: '[role="radiogroup"][aria-label="Machine color"]', reason: "D17: el color de la máquina se edita en su página" },
        { app: ".mod > .chip", mockup: ".mod > span:last-child", reason: "G4: el estado del módulo es el chip uniforme (interruptor); «Open settings» sí se compara" },
      ],
      min: SIDEBAR_GATE,
    },
  },
  ...AJUSTES,
  PALETA,
  ...PALETA_ESTADOS,
  ESTADOS,
  AVISOS,
  { ...AVISOS, name: "notices-closed", state: "closed", clicks: ['button[aria-label="Close"] >> nth=0'] },
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
