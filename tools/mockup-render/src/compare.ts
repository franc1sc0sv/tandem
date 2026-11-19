import { readFileSync } from 'node:fs';
import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';

/** Rectangle in screenshot pixels. */
export interface Region {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface CompareOptions {
  /**
   * Per-pixel color difference that is tolerated (0–1, pixelmatch's). 0.1 lets through
   * minimal color variations from Chromium's compositing.
   */
  threshold?: number;
  /** If `false` (the default), antialiased pixels do not count as different. */
  includeAA?: boolean;
  /** Side of the `worstTile` tiles, in pixels (default 64). */
  tile?: number;
  /**
   * Compares only this area of the two screenshots (for example the sidebar, T-112). The
   * `worstTile` coordinates remain those of the whole screenshot; `diffPng` is the size
   * of the area.
   */
  region?: Region;
}

export interface Tile extends Region {
  similarity: number;
}

export interface CompareResult {
  /** 1 = identical; fraction of pixels that match. */
  similarity: number;
  mismatched: number;
  total: number;
  width: number;
  height: number;
  /**
   * The `tile`×`tile` tile with the least similarity, over an overlapping grid (step `tile/2`).
   * Global similarity can stay ≥ 0.99 even if a button is missing or a chip changes (in the
   * mockups, changing state usually moves under 1 % of the pixels); this tile exposes
   * a local change like that.
   */
  worstTile: Tile;
  /** PNG with the differences in red over the dimmed expected image. */
  diffPng: Buffer;
}

export class SizeMismatchError extends Error {
  readonly actual: { width: number; height: number };
  readonly expected: { width: number; height: number };

  constructor(actual: { width: number; height: number }, expected: { width: number; height: number }) {
    super(`different sizes: ${actual.width}×${actual.height} versus ${expected.width}×${expected.height}`);
    this.name = 'SizeMismatchError';
    this.actual = actual;
    this.expected = expected;
  }
}

const DIFF_COLOR: [number, number, number] = [255, 0, 0];

/**
 * Tile starts along an axis of `length` pixels: every `size/2` and one flush with the final
 * edge, so all tiles are the same size and none is left out. With a fixed grid, a
 * `size`×`size` change that spans four tiles looks diluted to ~25 % in each; with
 * step `size/2`, some tile has at least half of each side of the change.
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
 * The least similar tile of a mask of different pixels (1 = different), over the
 * overlapping grid of `tileStarts`. With prefix sums each tile costs O(1) even if they
 * solapen.
 */
export function worstTileOf(mask: Uint8Array, width: number, height: number, size: number): Tile {
  const w = Math.min(size, width);
  const h = Math.min(size, height);
  let worst: Tile = { x: 0, y: 0, width: w, height: h, similarity: 1 };
  if (w === 0 || h === 0) return worst;
  // sum[y * stride + x] = different pixels in [0, x) × [0, y)
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

// pixelmatch paints each different pixel exactly DIFF_COLOR; identical ones stay grey
// (r = g = b) and antialiased ones yellow, so only the pure reds are counted.
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
      `invalid region: x=${r.x} y=${r.y} ${r.width}×${r.height} in a screenshot of ${width}×${height} ` +
        '(integers, size > 0 and inside the image)',
    );
  }
}

/**
 * Compares an app screenshot with the mockup's. Similarity is the fraction of
 * identical pixels (with pixelmatch's color and antialias tolerance); DESIGN §12 asks for ≥ 0.99.
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
