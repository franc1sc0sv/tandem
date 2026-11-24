# mockup-render

Renders the `mockups/v03/*.dc.html` mockups without the original runtime, takes screenshots into
`docs/mockups-png/` and compares screenshots (DESIGN §12: every screen ≥ 99 % similar to its mockup).

```sh
cd tools/mockup-render
pnpm install
pnpm exec playwright install chromium   # in CI: --with-deps chromium

pnpm mockups:serve                       # http://127.0.0.1:4173/ to view and browse them
pnpm mockups:shot                        # all screenshots → docs/mockups-png
pnpm mockups:shot --only V3-Arrangement  # only one mockup (and its states)
pnpm mockups:compare app.png ../../docs/mockups-png/V3-Desk.png --diff diff.png
pnpm test                                # runtime, comparison and the 23 mockups
```

## Pieces

- **`support.js`**: own runtime for the format (`<x-dc>`, `<helmet>`, `{{expr}}`, `<sc-for>`,
  `<sc-if>`, `Component extends DCLogic` with `state`, `setState` and `renderVals()`, `data-props`).
  It reads the template as inert text (with `<plaintext>` + DOMParser) so attributes such as
  `d="{{…}}"` do not raise console errors, and on every `setState` it patches the DOM instead of
  replacing it, so transitions run and an `<input>` does not lose focus. What cannot be
  evaluated ends up in `window.__dc.errors`, and expressions that give `undefined` in `window.__dc.missing`.
- **`src/states.ts`**: the named states of each mockup (click or typing steps).
  Each state yields `docs/mockups-png/<mockup>--<state>.png`.
- **`src/render.ts`**: opens each mockup in headless Chromium at the size of its `$preview`
  (1280×860, or 380×600 for the tray), waits for entrance animations, applies the steps and captures.
  Infinite ones (pulses, glows) are removed for the capture (`animations: 'disabled'`): the
  element's base style remains.
- **`src/compare.ts`**: `compare(app, mockup) → { similarity, worstTile, diffPng, … }` with
  pixelmatch (color tolerance 0.1, not counting antialiased pixels). `worstTile` is the
  64×64 tile with the least similarity, over an overlapping grid (32 px step, and tiles
  flush with the edges): changing a chip or a state usually moves under 1 % of the pixels,
  so global similarity alone does not detect it, and with a fixed grid a change that spans
  four tiles got diluted to ~25 % in each. `region` compares only an area.
- **`src/browser.ts`**: `launchBrowser`, `capture`, `settle`, `installRoutes`, `waitForMockup` and
  `checkPage`.
- **`src/harness.ts`**: the API for the app (below).

## Deterministic screenshots

- No network: the mockups, `support.js` and the fonts are served from Node inside the browser.
  Inter and JetBrains Mono come from `@fontsource/*` (the same ones the mockups' Google Fonts
  stylesheet asks for). What those subsets lack (⌃ ⌥ ⌘ → ↵ ≈ ✓ ➜) comes from DejaVu Sans,
  declared as an extra face of the same families only for those blocks; otherwise each machine
  draws it with its system font. A test requires that every non-ASCII character that can be
  seen (the HTML text, the CSS `content:` values and the `renderVals()` texts) is
  covered. Any other external request is blocked and counts as an error.
- Chromium is opened with `--disable-partial-raster` (`CHROMIUM_ARGS`, `launchBrowser()`). Without it
  Chromium re-rasterizes only the invalidated part of each tile, and the final PNG depends
  on which intermediate frames it got to paint during a transition: in `V3-Arrangement` the screen
  with `opacity: .4` came out with ±1 of color depending on the run (up to ~10,600 different pixels in
  `side-top-center`, 12–13 different screenshots between two runs).
- On the same machine, two runs give bit-identical PNGs (the test `the same screenshot comes out
  bit-identical across different runs` requires it for the `V3-Arrangement` states that used to vary). Between WSL
  and the CI job `ui (mockups)` they also came out identical (sha1 of the 72 in the `mockups-png`
  artifact of PR #70 = the committed ones), but that is not required: CI compares them with
  `compare()`'s tolerance (≥ 0.999), in case another runner image rasterizes differently. The
  references are the Chromium-on-Linux (WSL) ones.
- If a mockup or a state changes: `pnpm mockups:shot` and commit `docs/mockups-png/`. The
  test `docs/mockups-png has exactly one screenshot per mockup and state` warns if there are extras
  or missing ones. `mockups:shot` without `--only` deletes the screenshots that no longer match any
  mockup or state, but only if nothing failed.

## Using it from the app (T-112…T-120)

`tandem-ui` does not share a pnpm workspace with this folder. It depends on it with
`"@tandem/mockup-render": "link:../../tools/mockup-render"` (and `pnpm install` also in
`tools/mockup-render`, which brings Playwright, pixelmatch and pngjs). The harness entry point is
`@tandem/mockup-render/harness`, which does not load Playwright when imported:

```ts
import { capture, compareWithMockup, launchBrowser } from '@tandem/mockup-render/harness';

const browser = await launchBrowser();              // same arguments as the mockups
const page = await browser.newPage({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 1 });
await page.goto('http://127.0.0.1:1420/?demo=desk');
const png = await capture(page, 'docs/screenshots/v03/desk.png'); // settle + capture

const r = compareWithMockup(png, {
  mockup: 'V3-Desk',                // docs/mockups-png/V3-Desk.png
  state: undefined,                 // or 'scene-game' → V3-Desk--scene-game.png
  diffPath: 'out/V3-Desk-diff.png',
  region: { x: 0, y: 0, width: 240, height: 860 }, // optional: only the sidebar
});
r.similarity;            // 0–1; DESIGN §12 asks for ≥ 0.99
r.worstTile;             // { x, y, width, height, similarity } of the worst 64×64 tile
r.expectedPath;          // the reference used
```

- `capture(page, path?)` waits with `settle` (fonts loaded and finite animations finished) and captures
  with infinite animations removed and no text caret, like the mockups.
- `compareWithMockup` throws `MissingMockupShotError` if there is no reference (misspelled name or
  state) and `SizeMismatchError` if the sizes do not match.
- The `compare()` options (`threshold`, `includeAA`, `tile`, `region`) apply too.
- `@tandem/mockup-render` (without `/harness`) exports everything, including `renderMockups`.
