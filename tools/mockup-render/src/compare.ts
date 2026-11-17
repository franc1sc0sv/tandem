import { readFileSync } from 'node:fs';
import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';

/** Rectángulo en píxeles de la captura. */
export interface Region {
  x: number;
  y: number;
  width: number;
  height: number;
}

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
  /**
   * Compara solo esta zona de las dos capturas (por ejemplo la barra lateral, T-112). Las
   * coordenadas de `worstTile` siguen siendo las de la captura entera; `diffPng` es del tamaño
   * de la zona.
   */
  region?: Region;
}

export interface Tile extends Region {
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
   * El cuadro de `tile`×`tile` con menos parecido, sobre una rejilla solapada (paso `tile/2`).
   * La similitud global puede quedar ≥ 0.99 aunque falte un botón o cambie un chip (en los
   * mockups, cambiar de estado suele mover menos del 1 % de los píxeles); este cuadro deja ver
   * un cambio local así.
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

/**
 * Inicios de los cuadros sobre un eje de `length` píxeles: cada `size/2` y uno pegado al borde
 * final, así todos los cuadros miden lo mismo y ninguno queda afuera. Con una rejilla fija, un
 * cambio de `size`×`size` que cruza cuatro cuadros se ve diluido al ~25 % en cada uno; con
 * paso `size/2`, algún cuadro tiene al menos la mitad de cada lado del cambio.
 */
export function tileStarts(length: number, size: number): number[] {
  if (length <= size) return [0];
  const step = Math.max(1, Math.floor(size / 2));
  const starts: number[] = [];
  for (let s = 0; s + size <= length; s += step) starts.push(s);
  if (starts[starts.length - 1] !== length - size) starts.push(length - size);
  return starts;
}

/**
 * El cuadro con menos parecido de una máscara de píxeles distintos (1 = distinto), sobre la
 * rejilla solapada de `tileStarts`. Con sumas acumuladas cada cuadro cuesta O(1) aunque se
 * solapen.
 */
export function worstTileOf(mask: Uint8Array, width: number, height: number, size: number): Tile {
  const w = Math.min(size, width);
  const h = Math.min(size, height);
  let worst: Tile = { x: 0, y: 0, width: w, height: h, similarity: 1 };
  if (w === 0 || h === 0) return worst;
  // sum[y * stride + x] = píxeles distintos en [0, x) × [0, y)
  const stride = width + 1;
  const sum = new Uint32Array(stride * (height + 1));
  for (let y = 0; y < height; y++) {
    let row = 0;
    for (let x = 0; x < width; x++) {
      row += mask[y * width + x] ?? 0;
      sum[(y + 1) * stride + x + 1] = (sum[y * stride + x + 1] ?? 0) + row;
    }
  }
  const at = (x: number, y: number) => sum[y * stride + x] ?? 0;
  for (const ty of tileStarts(height, size)) {
    for (const tx of tileStarts(width, size)) {
      const bad = at(tx + w, ty + h) - at(tx, ty + h) - at(tx + w, ty) + at(tx, ty);
      const similarity = 1 - bad / (w * h);
      if (similarity < worst.similarity) worst = { x: tx, y: ty, width: w, height: h, similarity };
    }
  }
  return worst;
}

// pixelmatch pinta cada píxel distinto exactamente de DIFF_COLOR; los iguales quedan en gris
// (r = g = b) y los de antialias en amarillo, así que se cuentan solo los rojos puros.
function mismatchMask(diff: PNG): Uint8Array {
  const mask = new Uint8Array(diff.width * diff.height);
  for (let p = 0; p < mask.length; p++) {
    const i = p * 4;
    if (diff.data[i] === 255 && diff.data[i + 1] === 0 && diff.data[i + 2] === 0) mask[p] = 1;
  }
  return mask;
}

function load(img: Buffer | string): PNG {
  return PNG.sync.read(typeof img === 'string' ? readFileSync(img) : img);
}

function crop(png: PNG, r: Region): PNG {
  const out = new PNG({ width: r.width, height: r.height });
  PNG.bitblt(png, out, r.x, r.y, r.width, r.height, 0, 0);
  return out;
}

function checkRegion(r: Region, width: number, height: number): void {
  const ints = [r.x, r.y, r.width, r.height].every((v) => Number.isInteger(v));
  if (!ints || r.x < 0 || r.y < 0 || r.width <= 0 || r.height <= 0 || r.x + r.width > width || r.y + r.height > height) {
    throw new RangeError(
      `región inválida: x=${r.x} y=${r.y} ${r.width}×${r.height} en una captura de ${width}×${height} ` +
        '(enteros, tamaño > 0 y dentro de la imagen)',
    );
  }
}

/**
 * Compara una captura de la app con la del mockup. La similitud es la fracción de píxeles
 * iguales (con la tolerancia de color y de antialias de pixelmatch); DESIGN §12 pide ≥ 0.99.
 */
export function compare(actual: Buffer | string, expected: Buffer | string, opts: CompareOptions = {}): CompareResult {
  let a = load(actual);
  let b = load(expected);
  if (a.width !== b.width || a.height !== b.height) {
    throw new SizeMismatchError({ width: a.width, height: a.height }, { width: b.width, height: b.height });
  }
  const region = opts.region;
  if (region) {
    checkRegion(region, a.width, a.height);
    a = crop(a, region);
    b = crop(b, region);
  }
  const { width, height } = a;
  const diff = new PNG({ width, height });
  const mismatched = pixelmatch(a.data, b.data, diff.data, width, height, {
    threshold: opts.threshold ?? 0.1,
    includeAA: opts.includeAA ?? false,
    diffColor: DIFF_COLOR,
  });
  const total = width * height;
  const tile = worstTileOf(mismatchMask(diff), width, height, Math.max(1, Math.floor(opts.tile ?? 64)));
  return {
    similarity: total === 0 ? 1 : 1 - mismatched / total,
    mismatched,
    total,
    width,
    height,
    worstTile: region ? { ...tile, x: tile.x + region.x, y: tile.y + region.y } : tile,
    diffPng: PNG.sync.write(diff),
  };
}
