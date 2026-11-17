// API para el arnés de capturas de la app (T-112) y las pruebas de pantalla (T-113…T-120):
// `import { … } from '@tandem/mockup-render/harness'`. No carga Playwright al importarse
// (solo `launchBrowser` lo abre), así que sirve también en pruebas unitarias con PNG ya hechos.
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { compare, type CompareOptions, type CompareResult } from './compare.ts';
import { SHOTS_DIR, shotName } from './paths.ts';

export { CHROMIUM_ARGS, capture, launchBrowser, settle } from './browser.ts';
export { compare, SizeMismatchError, type CompareOptions, type CompareResult, type Region, type Tile } from './compare.ts';
export { SHOTS_DIR, shotName } from './paths.ts';

/** No hay captura de referencia para ese mockup o estado. */
export class MissingMockupShotError extends Error {
  readonly path: string;

  constructor(path: string) {
    super(`no existe la captura del mockup ${path} (¿nombre o estado mal escrito? se generan con \`pnpm mockups:shot\`)`);
    this.name = 'MissingMockupShotError';
    this.path = path;
  }
}

/**
 * Ruta de la captura de referencia: `<shotsDir>/<mockup>.png` o `<mockup>--<estado>.png`.
 * Lanza `MissingMockupShotError` si no existe.
 */
export function mockupShotPath(mockup: string, state?: string, shotsDir: string = SHOTS_DIR): string {
  const path = join(shotsDir, shotName(mockup, state));
  if (!existsSync(path)) throw new MissingMockupShotError(path);
  return path;
}

export interface MockupCompareOptions extends CompareOptions {
  /** Nombre del mockup, sin `.dc.html` (por ejemplo `V3-Puesto`). */
  mockup: string;
  /** Estado con nombre de `states.ts` (por ejemplo `escena-juego`); sin esto, el inicial. */
  state?: string;
  /** Dónde guardar el PNG de diferencias (se crea la carpeta). */
  diffPath?: string;
  /** Carpeta de las capturas de referencia (por omisión `docs/mockups-png`). */
  shotsDir?: string;
}

export interface MockupCompareResult extends CompareResult {
  /** Captura de referencia contra la que se comparó. */
  expectedPath: string;
  /** Dónde quedó el PNG de diferencias, si se pidió `diffPath`. */
  diffPath?: string;
}

/**
 * Compara una captura de la app (PNG en memoria o ruta) con la del mockup en
 * `docs/mockups-png/<mockup>[--<estado>].png`.
 *
 * ```ts
 * const r = compareWithMockup(await capture(page), { mockup: 'V3-Puesto', diffPath: 'out/puesto-diff.png' });
 * expect(r.similarity).toBeGreaterThanOrEqual(0.99);           // DESIGN §12
 * expect(r.worstTile.similarity).toBeGreaterThanOrEqual(0.9);  // un cambio local no pasa desapercibido
 * ```
 *
 * Lanza `MissingMockupShotError` si no hay referencia y `SizeMismatchError` si los tamaños no
 * coinciden (la app tiene que capturarse al `$preview` del mockup, 1280×860 casi siempre).
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
