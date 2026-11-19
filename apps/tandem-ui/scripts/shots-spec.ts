// What the harness captures (`pnpm shots`): each app screen in demo mode, which
// mockup it is compared against and which regions are measured separately.
//
// The differences allowed against the mockup (`mockups/README.md`, "What the mockups say and
// DECISIONS corrects") are masked in both images before comparing: `ignore` gives the
// element selector in the mockup and the reason.

import { PNG } from "pngjs";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Ignore {
  /** Selector of the element in the mockup whose box is masked in both screenshots. */
  mockupSelector?: string;
  /** Or a fixed box (only when no element delimits it). */
  rect?: Rect;
  reason: string;
}

export interface Region {
  name: string;
  /** Fixed box, or that of a selector in the mockup. */
  rect?: Rect;
  mockupSelector?: string;
  /** Thresholds that make `--check` fail (when given). */
  min?: { similarity: number; worstTile: number };
}

export interface ShotSpec {
  /** File name: `docs/screenshots/v03/<name>.png`. */
  name: string;
  /** App page (`index.html` or `tray.html`). */
  page: "index.html" | "tray.html";
  /** Demo mode parameters, without `?`. */
  query: string;
  hash: string;
  mockup: string;
  width: number;
  height: number;
  /** Fields to fill before the clicks (the scene name). */
  fills?: { selector: string; text: string }[];
  /** Keys to press at the end, after the clicks (open the palette, record a shortcut). */
  keys?: string[];
  /** Selectors to click before capturing (the steps of the mockup state). */
  clicks?: string[];
  /** Move the mouse out of the window after the clicks: the mockup redraws the button and loses the hover. */
  unhover?: boolean;
  /** Named mockup state (`docs/mockups-png/<mockup>--<state>.png`). */
  state?: string;
  /** Threshold for the whole screen (when given, `--check` requires it). */
  min?: { similarity: number; worstTile: number };
  ignore?: Ignore[];
  regions?: Region[];
  /** Compare only the content, when the mockup has no sidebar (see `Content`). */
  content?: Content;
}

/** A region masked in both screenshots: the `app` box in the app and the `mockup` box in the mockup. */
export interface ContentMask {
  app: string;
  mockup?: string;
  /** The allowed difference from `mockups/README.md` that justifies it. */
  reason: string;
}

/**
 * Content comparison for a flow mockup without a sidebar (`F-MachineMac`): the app
 * is captured at `appWidth` wide, so its content measures the same as the mockup's, and
 * both are cropped from the `app`/`mockup` corner down to the bottom edge.
 */
export interface Content {
  appWidth: number;
  /**
   * CSS added to the app only for this capture. With the flexible sidebar, its width
   * (and the content's left edge) lands on a fraction of a pixel and all the text looks
   * shifted by half a pixel; it is fixed at 220 px, outside the compared region.
   */
  appStyle?: string;
  app: string;
  mockup: string;
  masks: ContentMask[];
  min: { similarity: number; worstTile: number };
}

/** The sidebar of the main screens (V3-Desk, V3-Arrangement, V3-Network…). */
export const SIDEBAR: Region = { name: "sidebar", mockupSelector: 'nav[aria-label="Main"]' };

/** D1: there is no third machine, so the sidebar has no "Add machine". */
export const NO_ADD_MACHINE: Ignore = {
  mockupSelector: 'nav[aria-label="Main"] a[href="F-AddHost.dc.html"]',
  reason: "D1: no \"Add machine\" (exactly two machines)",
};

/** T-112 criterion: the sidebar equal to that of `V3-Desk`. */
export const SIDEBAR_GATE = { similarity: 0.99, worstTile: 0.97 };

const W = 1280;
/**
 * Window width at which the app content (sidebar fixed at 220 px and the
 * `main` padding, 2 × 28) measures the same as that of `F-MachineMac` (1280 − 2 × 32 = 1216 px).
 */
export const MAC_PAGE_WIDTH = 220 + 2 * 28 + 1216;
const H = 860;
const DEMO = "demo&still";

/** Acceptance criterion of each screen (ACCEPTANCE: similarity and worst tile). */
export const SCREEN_GATE = { similarity: 0.99, worstTile: 0.97 };

// T-117: the initial arrangement, the other machine on each side × each alignment (with the same
// clicks as the mockup), each chosen screen and the crossing test.
const ARRANGEMENT_SIDES = [
  ["top", "Top"],
  ["bottom", "Bottom"],
  ["left", "Left"],
  ["right", "Right"],
] as const;
const ARRANGEMENT_ALIGNS = ["start", "center", "end"] as const;
const arrangementShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/arrangement",
  mockup: "V3-Arrangement",
  width: W,
  height: H,
  ignore: [NO_ADD_MACHINE],
  regions: [SIDEBAR],
  min: SCREEN_GATE,
  ...extra,
});
/** The "Simulate…" buttons exist only in the mockup (`mockups/README.md`): they are masked. */
const SIMULATE_BUTTONS: Ignore[] = [
  { mockupSelector: 'button.sim:has-text("Simulate crossing")', reason: "\"Simulate crossing\" exists only in the mockup" },
  { mockupSelector: 'button.sim:has-text("Simulate no crossing")', reason: "\"Simulate no crossing\" exists only in the mockup" },
];
const crossingShot = (name: string, cross: string, state?: string): ShotSpec => ({
  name,
  page: "index.html",
  query: `${DEMO}${cross ? `&cross=${cross}` : ""}`,
  hash: "#/arrangement/crossing",
  mockup: "F-CrossingTest",
  state,
  width: W,
  height: H,
  ...(state ? {} : { ignore: SIMULATE_BUTTONS }),
  min: SCREEN_GATE,
});
export const ARRANGEMENT: ShotSpec[] = [
  arrangementShot("arrangement"),
  ...ARRANGEMENT_SIDES.flatMap(([side, label]) =>
    ARRANGEMENT_ALIGNS.map((al, i) =>
      arrangementShot(`arrangement-${side}-${al}`, { state: `side-${side}-${al}`, clicks: [`.slot[aria-label="${label}"]`, `.seg .sb >> nth=${i}`] }),
    ),
  ),
  arrangementShot("arrangement-screen-mac", { state: "screen-mac", clicks: ['.scr:has-text("MacBook Pro")'] }),
  arrangementShot("arrangement-unsupported", { state: "screen-unsupported", clicks: ['.scr:has-text("Odyssey G5 32")'] }),
  crossingShot("crossing", ""),
  crossingShot("crossing-crossed", "ok", "crossed"),
  crossingShot("crossing-no-cross", "timeout", "no-cross"),
  // Wrong edge: the mockup does not draw it (ACCEPTANCE: same components, no threshold).
  { ...crossingShot("crossing-other-edge", "wrong", "no-cross"), min: undefined },
];

// T-116: Monitors (with each input chosen), the scene editor and the progress of a scene.
const monitorsShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/monitors",
  mockup: "V3-Monitors",
  width: W,
  height: H,
  ignore: [NO_ADD_MACHINE],
  regions: [SIDEBAR],
  min: SCREEN_GATE,
  ...extra,
});
/** The editor fills in the same as the mockup: "Game", the ⌃⌥4 shortcut and what the 34" shows. */
const sceneShot = (name: string, pick: string, state?: string): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/monitors/scene",
  mockup: "F-NewScene",
  state,
  width: W,
  height: H,
  fills: [{ selector: 'input:not([type="hidden"])', text: "Game" }],
  clicks: [`[role="radio"]:has-text("${pick}")`, "button.kbd"],
  keys: ["Control+Alt+Digit4"],
  min: SCREEN_GATE,
});
/** The mockup's "Simulate…" buttons (mockup only); in "rollback" they move down with the error text. */
const SIMULATE: Ignore = { mockupSelector: "section.card:nth-of-type(1) > div:last-of-type", reason: "Simulate…: buttons exist only in the mockup" };
/** "Stop waiting": the agent has no way to cancel a power-on (UiCommand), so the app does not offer the action. */
const NO_CANCEL: Ignore = { mockupSelector: "section.card:nth-of-type(2) > div:last-of-type", reason: "Stop waiting: no cancel command in the IPC, so it is not shown" };
/** The mockup writes "of the MacBook"; the app shows the real machine name (D10). */
const MACHINE_NAME: Ignore = { rect: { x: 236, y: 228, width: 90, height: 22 }, reason: "\"of the MacBook\": the app shows the real machine name" };
const progressShot = (name: string, txn: "run" | "ok" | "fail", state?: string): ShotSpec => ({
  name,
  page: "index.html",
  query: `${DEMO}&txn=${txn}`,
  hash: "#/progress",
  mockup: "F-Transactions",
  state,
  width: W,
  height: H,
  ignore: [SIMULATE, NO_CANCEL, ...(txn === "fail" ? [] : [MACHINE_NAME])],
  min: SCREEN_GATE,
});
// T-114: Tunnels against `V3-Compute` in each state it draws (with the same clicks as the
// mockup). With no tunnels and with 12 processes the mockup does not draw them: they are captured with the same
// componentes y tokens, sin umbral (ACCEPTANCE, P2).
const tunnelsShot = (name: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/tunnels",
  mockup: "V3-Compute",
  width: W,
  height: H,
  ignore: [NO_ADD_MACHINE],
  regions: [SIDEBAR],
  min: SCREEN_GATE,
  ...extra,
});
export const TUNNELS: ShotSpec[] = [
  tunnelsShot("tunnels"),
  tunnelsShot("tunnels-process-two", { state: "process-2", clicks: ['[role="option"] >> nth=1'] }),
  tunnelsShot("tunnels-two-processes", { state: "tunnel-2-processes", clicks: ['.tun:has-text("T-101")'] }),
  tunnelsShot("tunnels-no-processes", { state: "tunnel-no-processes", clicks: ['.tun:has-text("Review PR #66")'] }),
  tunnelsShot("tunnels-lora", { state: "tunnel-lora", clicks: ['.tun:has-text("Train style LoRA")'] }),
  tunnelsShot("tunnels-closed", { state: "tunnel-closed", clicks: ['.tun:has-text("ComfyUI")'] }),
  tunnelsShot("tunnels-project-collapsed", { state: "project-collapsed", clicks: ['.proj:has-text("tandem")'] }),
  tunnelsShot("tunnels-empty", { query: `${DEMO}&tunnels=none`, min: undefined }),
  tunnelsShot("tunnels-twelve-processes", { query: `${DEMO}&tunnels=many`, min: undefined }),
];

export const MONITORS: ShotSpec[] = [
  monitorsShot("monitors"),
  monitorsShot("monitors-input-mac", { state: "input-mac", clicks: ['[role="radio"]:has-text("MacBook Pro")'] }),
  sceneShot("scene-new", "PC-Desktop"),
  sceneShot("scene-shows-mac", "MacBook Pro", "shows-mac"),
  sceneShot("scene-unchanged", "Keep as is", "unchanged"),
  progressShot("progress-in-progress", "run"),
  progressShot("progress-success", "ok", "success"),
  progressShot("progress-rollback", "fail", "rollback"),
];

/** The empty gap D1 leaves in the row of machine cards of `V3-Desk`. */
const NO_ADD_CARD: Ignore = { mockupSelector: 'a[href="F-AddHost.dc.html"].card', reason: "D1: no \"Add machine\" card" };

/** D2: the scaled desks take each machine's color; with two of the same system the background cannot match the mockup. */
const D2_DESKS: Ignore = { mockupSelector: 'div:has(> div[role="img"][aria-label^="To scale"])', reason: "D2: the machines' color changes the background of the desks" };

// T-120: Settings (one screenshot per tab) and the trouble states. Neither mockup has a
// sidebar (G3/D16): the content is compared, from the corner of its title.
const NONE = "#no-counterpart-in-the-app";
const settingsShot = (name: string, state: string | undefined, clicks: string[], masks: ContentMask[] = []): ShotSpec => ({
  name,
  page: "index.html",
  query: DEMO,
  hash: "#/settings",
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
export const SETTINGS: ShotSpec[] = [
  settingsShot("settings", undefined, [], [
    { app: ".pane > .row:nth-child(1) > .mut", mockup: ".pane > .row:nth-child(1) > input", reason: "The desk name is text: the mockup's editable field has no function here (§3.8)" },
    { app: NONE, mockup: 'button.danger:has-text("Remove from desk")', reason: "D1: no \"Remove from desk\" (exactly two machines)" },
    { app: ".pane > .row:nth-child(3) > button", mockup: ".pane > .row:nth-child(3) > button.ic", reason: "Without \"Remove from desk\" next to it, the pencil of the other machine falls at the right edge of the row" },
    { app: ".pane > .row:nth-child(4) > .fill", mockup: ".pane > .row:nth-child(4) > span", reason: "D1: \"Change the other machine\" instead of \"Add a machine\"" },
    { app: ".pane > .row:nth-child(4) > button", mockup: ".pane > .row:nth-child(4) > a.btn", reason: "D1: \"Pair again\" instead of \"Show code\"" },
  ]),
  { ...settingsShot("settings-shortcuts", "shortcuts", [tab("Shortcuts")]), query: `${DEMO}&solopc` },
  settingsShot("settings-keyboard", "keyboard", [tab("Keyboard and mouse")]),
  settingsShot("settings-updates", "updates", [tab("Updates")]),
  { ...settingsShot("settings-updates-checked", "updates-checked", [tab("Updates"), 'button:has-text("Check now")']), unhover: true },
  settingsShot("settings-advanced", "advanced", [tab("Advanced")], [
    { app: ".pane > .row:nth-child(3) .path", mockup: ".pane > .row:nth-child(3) .mono", reason: "The agent does not yet count MCP server sessions" },
  ]),
];

export const STATES: ShotSpec = {
  name: "states",
  page: "index.html",
  // `?states` seeds the mockup's sample texts (names, figures): everything is compared.
  query: `${DEMO}&states`,
  hash: "#/states",
  mockup: "V3-States",
  width: W,
  height: H,
  content: {
    appWidth: 220 + 2 * 28 + (1280 - 2 * 28),
    appStyle: 'nav[aria-label="Main"] { flex: 0 0 220px !important; }',
    app: "main .head",
    mockup: "h1",
    masks: [
      { app: NONE, mockup: 'button.btn:has-text("Send over Wi-Fi anyway")', reason: "The agent has no \"send over Wi-Fi anyway\": no function, no button" },
      { app: "article.st:nth-child(1) p", mockup: "article.st:nth-child(1) p", reason: "D1: the sentence does not say \"Add another machine\" (T-142)" },
      { app: "article.st:nth-child(1) .act button", mockup: "article.st:nth-child(1) a.btn", reason: "D1: \"Pair the other machine\" instead of \"Add machine\" (T-142)" },
    ],
    min: SIDEBAR_GATE,
  },
};

/**
 * T-120: the palette against `F-Palette`. The rows with desk data are masked (the mockup
 * carries sample scenes, tunnels and colors) and so is the background page, which in the app is the real one and
 * en el mockup un esqueleto.
 */
const ROW = (text: string) => `section[role="dialog"] .it:has(span:text-is("${text}"))`;
export const PALETTE: ShotSpec = {
  name: "palette",
  page: "index.html",
  query: `${DEMO}&palette`,
  hash: "#/desk",
  mockup: "F-Palette",
  width: W,
  height: H,
  keys: ["Meta+k"],
  ignore: [
    { mockupSelector: ROW("Add machine…"), reason: "D1: there is no third machine; the other one is changed" },
  ],
  regions: [{ name: "dialog", mockupSelector: 'section[role="dialog"]', min: SCREEN_GATE }],
  min: SCREEN_GATE,
};

/** T-144: the `F-Palette` states typed into the field (key by key, like the mockup, which filters as you type). */
const PALETTE_TYPE = (state: string, text: string): ShotSpec => ({
  ...PALETTE,
  name: `palette-${state}`,
  state,
  keys: ["Meta+k", ...text.split("")],
  regions: [{ name: "dialog", mockupSelector: 'section[role="dialog"]', min: SCREEN_GATE }],
});
export const PALETTE_STATES: ShotSpec[] = [PALETTE_TYPE("filter", "only"), PALETTE_TYPE("no-results", "zzz")];

/**
 * T-120: the tray against `V3-Tray`. The desk data replaces the sample data
 * (link, load, scenes and tunnels); since the real tunnels push the notice down, the
 * notice is masked too. "Lock here" does not exist in the app (DECISIONS, T-112).
 */
export const TRAY: ShotSpec = {
  name: "tray",
  page: "tray.html",
  // `tray` seeds the mockup data: 0.4 ms, the Mac at 18 %, "PC only" and three projects.
  query: `${DEMO}&tray`,
  hash: "",
  mockup: "V3-Tray",
  width: 380,
  height: 600,
  content: {
    appWidth: 380,
    app: ".pop",
    mockup: "x-dc > div",
    masks: [
      { app: NONE, mockup: '.lnk:has-text("Lock here")', reason: "Without the action in the IPC there is no button (T-112, G6)" },
    ],
    min: SCREEN_GATE,
  },
};

/**
 * `F-Notices`: the whole stack of notices (`?notices` seeds the mockup's three, with their texts,
 * tones and the first one's button) against the mockup's. To measure, the background page is hidden and
 * the stack (fixed to the window) becomes absolute: the mockup stacks it in the flow, inside a
 * box, and as a fixed layer the text lands on a different pixel (see `mockups/README.md`).
 */
export const NOTICES: ShotSpec = {
  name: "notices",
  page: "index.html",
  query: `${DEMO}&notices`,
  hash: "#/desk",
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

// T-119: the 6-step setup. The demo opens it at the requested step (`wizard=N`) with the mockup
// data (MacBook Pro as this machine, PC-Desktop as the other one).
const setupShot = (name: string, mockup: string, query: string, extra: Partial<ShotSpec> = {}): ShotSpec => ({
  name,
  page: "index.html",
  query: `${DEMO}&${query}`,
  hash: "#/desk",
  mockup,
  width: W,
  height: H,
  min: SCREEN_GATE,
  ...extra,
});
/** The "Simulate…" buttons exist only in the mockup (README): in the app those steps really happen. */
const SIMULATE_SETUP: Ignore[] = [
  { mockupSelector: 'button.sim:has-text("Simulate crossing")', reason: "«Simulate crossing» es solo del mockup" },
  { mockupSelector: 'button.sim:has-text("Simulate another edge")', reason: "\"Simulate another edge\" exists only in the mockup" },
];
/**
 * `F-Setup5`: its CSS declares `.cur` for the drawing cursor and the current list step
 * carries that class too (`.stl.cur`): the mockup takes it out of the flow (14 × 14, absolute) and "Done" moves up
 * 44 px. The app draws the list like the other screens; the two affected steps are masked.
 */
const SETUP5_LIST: Ignore[] = [
  { rect: { x: 10, y: 138, width: 140, height: 60 }, reason: "F-Setup5: the current step leaves the flow because of the duplicated .cur class" },
  { rect: { x: 0, y: 258, width: 300, height: 94 }, reason: "F-Setup5: \"Done\" moves up 44 px in the mockup because of the duplicated .cur class" },
];
/**
 * `F-Setup5` writes "of the MacBook to the PC"; the app shows the machine names. Only what follows
 * "the mouse of" on the second line is masked: the first line and the start of the second are the same.
 */
const SETUP5_LEAD: Ignore = { rect: { x: 438, y: 134, width: 552, height: 20 }, reason: "F-Setup5: the names of the desk's machines (\"the MacBook\", \"the PC\" in the mockup)" };
/** The verdict texts name the desk's machines (the mockup says "the MacBook"). */
const SETUP5_VERDICT: Ignore = { rect: { x: 350, y: 482, width: 440, height: 40 }, reason: "F-Setup5: the verdict names the desk's machines" };
const SETUP5: Ignore[] = [...SETUP5_LIST, SETUP5_LEAD];
/** `F-Setup3` writes "1 missing" and does not offer to restart the agent; the app agrees ("1 missing") and offers it (T-090). */
const PERMS_FOOT: Ignore = { rect: { x: 775, y: 770, width: 200, height: 40 }, reason: "F-Setup3: \"1 missing\" and \"Restart agent\" (only in the app)" };
const asisColor = (label: string, state: string) => setupShot(`setup-machine-${state}`, "F-Setup1", "wizard=0&admin=pending", { state, clicks: [`[role="radio"][aria-label="${label}"]`] });
export const SETUP: ShotSpec[] = [
  setupShot("setup-machine", "F-Setup1", "wizard=0&admin=pending"),
  asisColor("Violet", "violet"),
  asisColor("Teal", "teal"),
  asisColor("Amber", "amber"),
  setupShot("setup-desk", "V3-Setup", "wizard=1&peer=none&pair=code", {
    fills: ["4", "8", "2", "9"].map((text, i) => ({ selector: `input[aria-label="Digit ${i + 1}"]`, text })),
    // Focus jumps to the next digit by itself; the mockup has no focus on any.
    clicks: [".wiz h1"],
  }),
  // T-145: "Create a new desk". The panel (desk name in the mockup, code in the app) is masked entirely.
  setupShot("setup-desk-new", "V3-Setup", "wizard=1&peer=none&pair=code", {
    state: "new",
    clicks: ['[role="radio"]:has-text("Create a new desk")'],
    ignore: [{ rect: { x: 350, y: 315, width: 760, height: 205 }, reason: "T-145: the desk name does not exist in the agent (§3.8); the app shows the pairing code (T-097)" }],
  }),
  setupShot("setup-permissions", "F-Setup3", "wizard=2&perms=missing"),
  setupShot("setup-permissions-one", "F-Setup3", "wizard=2&perms=missing", { state: "one-granted", clicks: ['button:has-text("Open settings") >> nth=0'], ignore: [PERMS_FOOT] }),
  setupShot("setup-permissions-all", "F-Setup3", "wizard=2&perms=missing", {
    state: "all-granted",
    clicks: ['button:has-text("Open settings") >> nth=0', 'button:has-text("Open settings") >> nth=0'],
  }),
  setupShot("setup-keyboard", "F-Setup4", "wizard=3"),
  setupShot("setup-arrangement", "F-Setup5", "wizard=4", { ignore: SETUP5 }),
  setupShot("setup-arrangement-waiting", "F-Setup5", "wizard=4", { state: "waiting", clicks: ['button:has-text("Test crossing")'], ignore: [...SETUP5, SETUP5_VERDICT, ...SIMULATE_SETUP] }),
  setupShot("setup-arrangement-crossed", "F-Setup5", "wizard=4&cross=ok", { state: "crossed", clicks: ['button:has-text("Test crossing")'], ignore: SETUP5 }),
  setupShot("setup-arrangement-other-edge", "F-Setup5", "wizard=4&cross=wrong", { state: "other-edge", clicks: ['button:has-text("Test crossing")'], ignore: SETUP5 }),
  setupShot("setup-done", "F-Setup6", "wizard=5&crossed"),
];

export const SHOTS: ShotSpec[] = [
  {
    name: "desk",
    page: "index.html",
    query: DEMO,
    hash: "#/desk",
    mockup: "V3-Desk",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE, NO_ADD_CARD],
    regions: [{ ...SIDEBAR, min: SIDEBAR_GATE }],
    // T-113: the whole screen. Only the empty gap D1 removes ("Add machine") is masked; the
    // app has no hidden card (Power on/off live inside the other machine's card).
    min: SCREEN_GATE,
  },
  // D2 (scope of T-126): the same desk with two Windows and with two Macs. Names and
  // colors change, so there is no threshold: they serve to see each combination.
  // T-144: apply a scene from the Desk (the same clicks as `V3-Desk--scene-*`).
  ...[
    ["scene-macbook-only", "MacBook only"],
    ["scene-game", "Game"],
  ].map(([state, label]): ShotSpec => ({ name: `desk-${state}`, page: "index.html", query: DEMO, hash: "#/desk", mockup: "V3-Desk", state, width: W, height: H, clicks: [`.scene:has-text("${label}")`], ignore: [NO_ADD_MACHINE, NO_ADD_CARD], min: SCREEN_GATE })),
  { name: "desk-win-win", page: "index.html", query: "demo=win-win&still", hash: "#/desk", mockup: "V3-Desk", width: W, height: H, ignore: [NO_ADD_MACHINE, NO_ADD_CARD, D2_DESKS], regions: [SIDEBAR] },
  { name: "desk-mac-mac", page: "index.html", query: "demo=mac-mac&still", hash: "#/desk", mockup: "V3-Desk", width: W, height: H, ignore: [NO_ADD_MACHINE, NO_ADD_CARD, D2_DESKS], regions: [SIDEBAR] },
  ...TUNNELS,
  ...MONITORS,
  ...ARRANGEMENT,
  ...SETUP,
  // T-118: the whole Network screen against `V3-Network`. What D1 removes ("Add machine") is masked, and
  // the label of the chart window: the agent keeps 60 s, the mockup says "10 min".
  {
    name: "network",
    page: "index.html",
    query: DEMO,
    hash: "#/network",
    mockup: "V3-Network",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE, { mockupSelector: 'span.mut:has-text("Latency · 10 min")', reason: "T-102: the chart is 60 s (DESIGN §6), not 10 min" }],
    regions: [SIDEBAR],
    min: SCREEN_GATE,
  },
  // T-115: the other machine's page, with a threshold on the sidebar and on the content.
  {
    name: "machine-b",
    page: "index.html",
    query: DEMO,
    hash: "#/machine/b",
    mockup: "V3-Machine",
    width: W,
    height: H,
    ignore: [NO_ADD_MACHINE],
    regions: [
      { ...SIDEBAR, min: SIDEBAR_GATE },
      { name: "content", mockupSelector: "main", min: SIDEBAR_GATE },
    ],
  },
  // T-115: this machine against `F-MachineMac`, in the state it draws (one permission missing). Only
  // the content, with the differences from `mockups/README.md` masked.
  {
    name: "machine-a",
    page: "index.html",
    query: `${DEMO}&trouble=permissions`,
    hash: "#/machine/a",
    mockup: "F-MachineMac",
    width: W,
    height: H,
    content: {
      appWidth: MAC_PAGE_WIDTH,
      appStyle: 'nav[aria-label="Main"] { flex: 0 0 220px !important; }',
      app: "main header",
      mockup: "header",
      masks: [
        { app: '[role="radiogroup"][aria-label="Machine color"]', reason: "D17: the machine color is edited on its page" },
        { app: ".mod > .chip", mockup: ".mod > span:last-child", reason: "G4: the module state is the uniform chip (switch); \"Open settings\" is compared" },
      ],
      min: SIDEBAR_GATE,
    },
  },
  ...SETTINGS,
  PALETTE,
  ...PALETTE_STATES,
  STATES,
  NOTICES,
  { ...NOTICES, name: "notices-closed", state: "closed", clicks: ['button[aria-label="Close"] >> nth=0'] },
  TRAY,
];

// ---- Images -------------------------------------------------------------------------------

/** Paints `rects` in a fixed color (the regions that are not compared). */
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

/** Crops `rect` from the image (clamped to its edges). */
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

/** The app URL served by the harness. */
export function appUrl(origin: string, s: Pick<ShotSpec, "page" | "query" | "hash">): string {
  return `${origin}/${s.page}${s.query ? `?${s.query}` : ""}${s.hash}`;
}
