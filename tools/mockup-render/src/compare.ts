import { readFileSync } from 'node:fs';
import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';

export interface CompareOptions {
  /**
   * Diferencia de color por píxel que se tolera (0–1, la de pixelmatch). 0.1 deja pasar
   * variaciones mínimas de color de la composición de Chromium.
   */
  threshold?: number;
  /** Si es `false` (por omisión), los píxeles de antialias no cuentan como distintos. */
  includeAA?: boolean;
  /** Lado de los cuadros de `worstTile`, en píxeles (por omisión 64). */
  tile?: number;
}

export interface Tile {
  x: number;
  y: number;
  width: number;
  height: number;
  similarity: number;
}

export interface CompareResult {
  /** 1 = iguales; fracción de píxeles que coinciden. */
  similarity: number;
  mismatched: number;
  total: number;
  width: number;
  height: number;
  /**
   * El cuadro de `tile`×`tile` con menos parecido. La similitud global puede quedar ≥ 0.99
   * aunque falte un botón o cambie un chip (en los mockups, cambiar de estado suele mover
   * menos del 1 % de los píxeles); este cuadro deja ver un cambio local así.
   */
  worstTile: Tile;
  /** PNG con las diferencias en rojo sobre la imagen esperada atenuada. */
  diffPng: Buffer;
}

export class SizeMismatchError extends Error {
  readonly actual: { width: number; height: number };
  readonly expected: { width: number; height: number };

  constructor(actual: { width: number; height: number }, expected: { width: number; height: number }) {
    super(`tamaños distintos: ${actual.width}×${actual.height} contra ${expected.width}×${expected.height}`);
    this.name = 'SizeMismatchError';
    this.actual = actual;
    this.expected = expected;
  }
}

const DIFF_COLOR: [number, number, number] = [255, 0, 0];

// pixelmatch pinta cada píxel distinto exactamente de DIFF_COLOR; los iguales quedan en gris
// (r = g = b) y los de antialias en amarillo, así que se cuentan solo los rojos puros.
function worstTile(diff: PNG, size: number): Tile {
  let worst: Tile = { x: 0, y: 0, width: Math.min(size, diff.width), height: Math.min(size, diff.height), similarity: 1 };
  for (let ty = 0; ty < diff.height; ty += size) {
    for (let tx = 0; tx < diff.width; tx += size) {
      const w = Math.min(size, diff.width - tx);
      const h = Math.min(size, diff.height - ty);
      let bad = 0;
      for (let y = ty; y < ty + h; y++) {
        for (let x = tx; x < tx + w; x++) {
          const i = (y * diff.width + x) * 4;
          if (diff.data[i] === 255 && diff.data[i + 1] === 0 && diff.data[i + 2] === 0) bad += 1;
        }
      }
      const similarity = 1 - bad / (w * h);
      if (similarity < worst.similarity) worst = { x: tx, y: ty, width: w, height: h, similarity };
    }
  }
  return worst;
}

function load(img: Buffer | string): PNG {
  return PNG.sync.read(typeof img === 'string' ? readFileSync(img) : img);
}

/**
 * Compara una captura de la app con la del mockup. La similitud es la fracción de píxeles
 * iguales (con la tolerancia de color y de antialias de pixelmatch); DESIGN §12 pide ≥ 0.99.
 */
export function compare(actual: Buffer | string, expected: Buffer | string, opts: CompareOptions = {}): CompareResult {
  const a = load(actual);
  const b = load(expected);
  if (a.width !== b.width || a.height !== b.height) {
    throw new SizeMismatchError({ width: a.width, height: a.height }, { width: b.width, height: b.height });
  }
  const { width, height } = a;
  const diff = new PNG({ width, height });
  const mismatched = pixelmatch(a.data, b.data, diff.data, width, height, {
    threshold: opts.threshold ?? 0.1,
    includeAA: opts.includeAA ?? false,
    diffColor: DIFF_COLOR,
  });
  const total = width * height;
  return {
    similarity: total === 0 ? 1 : 1 - mismatched / total,
    mismatched,
    total,
    width,
    height,
    worstTile: worstTile(diff, Math.max(1, Math.floor(opts.tile ?? 64))),
    diffPng: PNG.sync.write(diff),
  };
}
