// Own set of line icons, copied from the v0.3 mockups (viewBox 24, stroke 1.8 in
// navigation and 2 in buttons). Each one is the interior of the <svg> and its ends.

export interface IconDef {
  body: string;
  linecap?: "round";
  linejoin?: "round";
  /** Filled with the color instead of stroked. */
  fill?: boolean;
}

export const ICONS = {
  desk: {
    body: '<rect x="3" y="3" width="7" height="7" rx="1.5"></rect><rect x="14" y="3" width="7" height="7" rx="1.5"></rect><rect x="3" y="14" width="7" height="7" rx="1.5"></rect><rect x="14" y="14" width="7" height="7" rx="1.5"></rect>',
  },
  tunnels: { body: '<path d="M3 12h4M17 12h4"></path><rect x="7" y="8" width="10" height="8" rx="4"></rect>', linecap: "round" },
  monitors: { body: '<rect x="3" y="4" width="18" height="12" rx="2"></rect><path d="M9 20h6M12 16v4"></path>' },
  arrangement: { body: '<rect x="2" y="7" width="9" height="10" rx="1.5"></rect><rect x="13" y="5" width="9" height="14" rx="1.5"></rect>' },
  network: { body: '<circle cx="5" cy="12" r="2.5"></circle><circle cx="19" cy="12" r="2.5"></circle><path d="M7.5 12h9"></path>' },
  settings: {
    body: '<circle cx="12" cy="12" r="3"></circle><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"></path>',
  },
  plus: { body: '<path d="M12 5v14M5 12h14"></path>' },
  search: { body: '<circle cx="11" cy="11" r="7"></circle><path d="m20 20-3.5-3.5"></path>' },
  edit: { body: '<path d="M4 20h4L19 9l-4-4L4 16z"></path><path d="m13.5 6.5 4 4"></path>', linejoin: "round" },
  laptop: { body: '<rect x="4" y="5" width="16" height="11" rx="1.5"></rect><path d="M2 19h20"></path>' },
  tower: { body: '<rect x="6" y="3" width="12" height="18" rx="2"></rect><circle cx="12" cy="15" r="2.5"></circle><path d="M9 7h6"></path>' },
  bolt: { body: '<path d="M13 2 4 14h7l-1 8 9-12h-7z"></path>', fill: true },
  check: { body: '<path d="m5 12 5 5L20 7"></path>', linecap: "round" },
  x: { body: '<path d="M6 6l12 12M18 6 6 18"></path>', linecap: "round" },
  warn: { body: '<path d="M12 3 2 20h20z"></path><path d="M12 10v4M12 17h.01"></path>' },
  info: { body: '<circle cx="12" cy="12" r="9"></circle><path d="M12 11v5M12 8h.01"></path>' },
  lock: { body: '<rect x="5" y="11" width="14" height="10" rx="2"></rect><path d="M8 11V7a4 4 0 0 1 8 0v4"></path>' },
  power: { body: '<path d="M12 3v8"></path><path d="M6.3 6.8a8 8 0 1 0 11.4 0"></path>', linecap: "round" },
  window: { body: '<rect x="3" y="4" width="18" height="16" rx="2"></rect><path d="M3 9h18"></path>' },
  "arrow-right": { body: '<path d="M5 12h14M13 6l6 6-6 6"></path>', linecap: "round", linejoin: "round" },
  "arrow-left": { body: '<path d="M19 12H5M11 6l-6 6 6 6"></path>', linecap: "round", linejoin: "round" },
  "chevron-down": { body: '<path d="m6 9 6 6 6-6"></path>', linecap: "round", linejoin: "round" },
  "chevron-right": { body: '<path d="m9 6 6 6-6 6"></path>' },
  // Arrangement (V3-Arrangement): arrows of the mini plan and alignments along the edge.
  "arrow-up": { body: '<path d="M12 19V5M6 11l6-6 6 6"></path>', linecap: "round", linejoin: "round" },
  "arrow-down": { body: '<path d="M12 5v14M6 13l6 6 6-6"></path>', linecap: "round", linejoin: "round" },
  "align-top": { body: '<path d="M4 4h16"></path><rect x="6" y="8" width="5" height="12" rx="1"></rect><rect x="13" y="8" width="5" height="7" rx="1"></rect>', linecap: "round" },
  "align-middle": { body: '<path d="M4 12h16"></path><rect x="6" y="5" width="5" height="14" rx="1"></rect><rect x="13" y="8" width="5" height="8" rx="1"></rect>', linecap: "round" },
  "align-bottom": { body: '<path d="M4 20h16"></path><rect x="6" y="4" width="5" height="12" rx="1"></rect><rect x="13" y="9" width="5" height="7" rx="1"></rect>', linecap: "round" },
  "align-left": { body: '<path d="M4 4v16"></path><rect x="8" y="6" width="12" height="5" rx="1"></rect><rect x="8" y="13" width="7" height="5" rx="1"></rect>', linecap: "round" },
  "align-center": { body: '<path d="M12 4v16"></path><rect x="5" y="6" width="14" height="5" rx="1"></rect><rect x="8" y="13" width="8" height="5" rx="1"></rect>', linecap: "round" },
  "align-right": { body: '<path d="M20 4v16"></path><rect x="4" y="6" width="12" height="5" rx="1"></rect><rect x="9" y="13" width="7" height="5" rx="1"></rect>', linecap: "round" },
  refresh: { body: '<path d="M20 11a8 8 0 0 0-14.9-3M4 13a8 8 0 0 0 14.9 3"></path><path d="M20 4v4h-4M4 20v-4h4"></path>', linecap: "round" },
  download: { body: '<path d="M12 4v11M7 10l5 5 5-5M5 20h14"></path>', linecap: "round" },
  external: { body: '<path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"></path>', linecap: "round" },
  login: { body: '<path d="M15 4h4a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1h-4M10 17l5-5-5-5M15 12H4"></path>', linecap: "round", linejoin: "round" },
  "grid-plus": {
    body: '<rect x="3" y="3" width="7" height="7" rx="1.5"></rect><rect x="14" y="3" width="7" height="7" rx="1.5"></rect><rect x="3" y="14" width="7" height="7" rx="1.5"></rect><path d="M17.5 14v7M14 17.5h7"></path>',
    linecap: "round",
  },
  skip: { body: '<path d="M5 5l7 7-7 7M13 5l7 7-7 7"></path>', linecap: "round", linejoin: "round" },
  copy: { body: '<rect x="9" y="9" width="12" height="12" rx="2"></rect><path d="M5 15V5a2 2 0 0 1 2-2h8"></path>' },
  mouse: { body: '<path d="M12 3a6 6 0 0 1 6 6v6a6 6 0 0 1-12 0V9a6 6 0 0 1 6-6zM12 7v4"></path>', linecap: "round" },
  sliders: { body: '<path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0M14 4v4M8 10v4M16 16v4"></path>', linecap: "round" },
  logout: { body: '<path d="M9 4H5a1 1 0 0 0-1 1v14a1 1 0 0 0 1 1h4M14 17l5-5-5-5M19 12H9"></path>', linecap: "round", linejoin: "round" },
  eye: { body: '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"></path><circle cx="12" cy="12" r="3"></circle>' },
  screen: { body: '<rect x="3" y="4" width="18" height="12" rx="2"></rect><path d="M9 20h6"></path>' },
  "wifi-fill": { body: '<path d="M12 20a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM6.3 13.3l1.4 1.4a6 6 0 0 1 8.6 0l1.4-1.4a8 8 0 0 0-11.4 0z"></path>', fill: true },
  "shield-plain": { body: '<path d="M12 3 5 6v5c0 4.5 3 8.3 7 10 4-1.7 7-5.5 7-10V6z"></path>' },
  sync: { body: '<path d="M4 12a8 8 0 0 1 14-5.3M20 12a8 8 0 0 1-14 5.3"></path><path d="M18 3v4h-4M6 21v-4h4"></path>' },
  folder: { body: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"></path>' },
  file: { body: '<path d="M6 3h8l4 4v14H6zM14 3v4h4"></path>' },
  terminal: { body: '<path d="m5 8 4 4-4 4M12 16h7"></path>', linecap: "round" },
  play: { body: '<path d="M8 5v14l11-7z"></path>', fill: true },
  shield: { body: '<path d="M12 3 5 6v5c0 4.5 3 8.3 7 10 4-1.7 7-5.5 7-10V6z"></path><path d="m9 12 2 2 4-4"></path>' },
  cursor: { body: '<path d="M5 3l14 8-6 2-2 6z"></path>', linejoin: "round" },
  wifi: { body: '<path d="M2.5 9a14 14 0 0 1 19 0M5.5 12.5a9 9 0 0 1 13 0M9 16a4 4 0 0 1 6 0"></path><path d="M12 19.5h.01"></path>', linecap: "round" },
  git: {
    body: '<circle cx="6" cy="6" r="2.5"></circle><circle cx="6" cy="18" r="2.5"></circle><circle cx="18" cy="8" r="2.5"></circle><path d="M6 8.5v7M18 10.5c0 4-6 3-11 5.5"></path>',
  },
  trash: { body: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"></path>', linecap: "round" },
  /** The pencil of "Edit" in V3-Monitors (without the `edit` line). */
  pencil: { body: '<path d="M4 20h4L19 9l-4-4L4 16z"></path>', linejoin: "round" },
  hand: { body: '<path d="M9 11V5a2 2 0 0 1 4 0v6M13 11V4a2 2 0 0 1 4 0v8M9 11a2 2 0 0 0-4 0v2a7 7 0 0 0 14 0v-1"></path>' },
  unplugged: { body: '<path d="M7 7l10 10M9 3v4M15 3v4M6 9h3M15 9h3"></path>' },
  plug: { body: '<path d="M9 2v6M15 2v6M6 8h12v4a6 6 0 0 1-12 0zM12 18v4"></path>' },
  pulse: { body: '<path d="M2 12h4l3-8 4 16 3-8h6"></path>' },
  cpu: { body: '<rect x="6" y="6" width="12" height="12" rx="2"></rect><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"></path>' },
  ram: { body: '<rect x="2" y="7" width="20" height="10" rx="1.5"></rect><path d="M6 17v3M10 17v3M14 17v3M18 17v3M6 11h2M10 11h2M14 11h2"></path>' },
  gpu: { body: '<rect x="2" y="6" width="20" height="12" rx="2"></rect><circle cx="9" cy="12" r="3"></circle><path d="M15 10h4M15 14h4M6 18v3"></path>' },
  disk: { body: '<ellipse cx="12" cy="6" rx="8" ry="3"></ellipse><path d="M4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6"></path>' },
  /** The VRAM notice triangle (without the `warn` dot). */
  alert: { body: '<path d="M12 3 2 20h20z"></path><path d="M12 10v4"></path>' },
  keyboard: { body: '<rect x="2" y="6" width="20" height="12" rx="2"></rect><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"></path>' },
  /** Speedometer (`Measure bandwidth`). */
  gauge: { body: '<path d="M4 15a8 8 0 1 1 16 0"></path><path d="m12 15 4-5"></path>', linecap: "round" },
  ethernet: { body: '<path d="M5 4h14v10h-3v3h-2v3h-4v-3H8v-3H5z"></path>', fill: true },
  sparkle: { body: '<path d="M12 2l1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8z"></path>', fill: true },
} satisfies Record<string, IconDef>;

export type IconName = keyof typeof ICONS;

/** The icon of a machine by its system (D2: by `os`, not by side). */
export function machineIcon(os: "macos" | "windows"): IconName {
  return os === "macos" ? "laptop" : "tower";
}
