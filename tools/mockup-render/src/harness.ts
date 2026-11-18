// API for the app screenshot harness (T-112) and the screen tests (T-113…T-120):
// `import { … } from '@tandem/mockup-render/harness'`. It does not load Playwright when imported
// (only `launchBrowser` opens it), so it also works in unit tests with ready-made PNGs.
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { compare, type CompareOptions, type CompareResult } from './compare.ts';
import { SHOTS_DIR, shotName } from './paths.ts';

export { CHROMIUM_ARGS, capture, launchBrowser, settle } from './browser.ts';
export { compare, SizeMismatchError, type CompareOptions, type CompareResult, type Region, type Tile } from './compare.ts';
export { SHOTS_DIR, shotName } from './paths.ts';

/** There is no reference screenshot for that mockup or state. */
export class MissingMockupShotError extends Error {
  readonly path: string;

  constructor(path: string) {
    super(`there is no mockup screenshot ${path} (misspelled name or state? they are generated with \`pnpm mockups:shot\`)`);
    this.name = 'MissingMockupShotError';
    this.path = path;
  }
}

/**
 * Path of the reference screenshot: `<shotsDir>/<mockup>.png` or `<mockup>--<state>.png`.
 * Throws `MissingMockupShotError` if it does not exist.
 */
export function mockupShotPath(mockup: string, state?: string, shotsDir: string = SHOTS_DIR): string {
  const path = join(shotsDir, shotName(mockup, state));
  if (!existsSync(path)) throw new MissingMockupShotError(path);
  return path;
}

export interface MockupCompareOptions extends CompareOptions {
  /** Mockup name, without `.dc.html` (for example `V3-Desk`). */
  mockup: string;
  /** Named state from `states.ts` (for example `scene-game`); without it, the initial one. */
  state?: string;
  /** Where to save the difference PNG (the folder is created). */
  diffPath?: string;
  /** Folder of the reference screenshots (default `docs/mockups-png`). */
  shotsDir?: string;
}

export interface MockupCompareResult extends CompareResult {
  /** Reference screenshot it was compared against. */
  expectedPath: string;
  /** Where the difference PNG ended up, if `diffPath` was requested. */
  diffPath?: string;
}

/**
 * Compares an app screenshot (in-memory PNG or path) with the mockup one in
 * `docs/mockups-png/<mockup>[--<state>].png`.
 *
 * ```ts
 * const r = compareWithMockup(await capture(page), { mockup: 'V3-Desk', diffPath: 'out/desk-diff.png' });
 * expect(r.similarity).toBeGreaterThanOrEqual(0.99);           // DESIGN §12
 * expect(r.worstTile.similarity).toBeGreaterThanOrEqual(0.9);  // a local change does not go unnoticed
 * ```
 *
 * Throws `MissingMockupShotError` if there is no reference and `SizeMismatchError` if the sizes do not
 * match (the app has to be captured at the mockup's `$preview`, 1280×860 almost always).
 */
export function compareWithMockup(actual: Buffer | string, opts: MockupCompareOptions): MockupCompareResult {
  const { mockup, state, diffPath, shotsDir, ...compareOpts } = opts;
  const expectedPath = mockupShotPath(mockup, state, shotsDir);
  const result = compare(actual, expectedPath, compareOpts);
  if (diffPath) {
    mkdirSync(dirname(diffPath), { recursive: true });
    writeFileSync(diffPath, result.diffPng);
  }
  return { ...result, expectedPath, ...(diffPath ? { diffPath } : {}) };
}
