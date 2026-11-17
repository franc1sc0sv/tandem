import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { PNG } from 'pngjs';
import { describe, expect, it } from 'vitest';
import { SizeMismatchError, compare } from '../src/compare.ts';
import { SHOTS_DIR } from '../src/paths.ts';

type Rgb = [number, number, number];

function image(width: number, height: number, color: (x: number, y: number) => Rgb): PNG {
  const png = new PNG({ width, height });
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const [r, g, b] = color(x, y);
      const i = (y * width + x) * 4;
      png.data[i] = r;
      png.data[i + 1] = g;
      png.data[i + 2] = b;
      png.data[i + 3] = 255;
    }
  }
  return png;
}

const encode = (png: PNG) => PNG.sync.write(png);

// Algo parecido a una pantalla: fondo oscuro con tarjetas.
const screen = (x: number, y: number): Rgb =>
  x % 300 > 20 && y % 200 > 20 ? [21, 22, 26] : [14 + (x % 7), 15, 18 + (y % 5)];

function withBlock(png: PNG, bx: number, by: number, w: number, h: number, rgb: Rgb): PNG {
  const out = new PNG({ width: png.width, height: png.height });
  png.data.copy(out.data);
  for (let y = by; y < by + h; y++) {
    for (let x = bx; x < bx + w; x++) {
      const i = (y * png.width + x) * 4;
      out.data[i] = rgb[0];
      out.data[i + 1] = rgb[1];
      out.data[i + 2] = rgb[2];
    }
  }
  return out;
}

// Generador con semilla: las pruebas de propiedades dan siempre lo mismo.
function rng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

describe('compare', () => {
  const base = image(1280, 860, screen);

  it('la misma imagen da 1.0', () => {
    const r = compare(encode(base), encode(base));
    expect(r.similarity).toBe(1);
    expect(r.mismatched).toBe(0);
    expect(r.worstTile.similarity).toBe(1);
  });

  it('un bloque cambiado baja de 0.99 y el peor cuadro cae sobre el bloque', () => {
    // 120×100 = 1.09 % de 1280×860
    const changed = withBlock(base, 500, 300, 120, 100, [122, 167, 230]);
    const r = compare(encode(changed), encode(base));
    expect(r.similarity).toBeLessThan(0.99);
    // Algunos bordes del bloque cuentan como antialias (no se cuentan): casi todo el bloque.
    expect(r.mismatched).toBeGreaterThan(0.98 * 120 * 100);
    expect(r.mismatched).toBeLessThanOrEqual(120 * 100);
    expect(r.worstTile.similarity).toBe(0);
    expect(r.worstTile.x).toBeGreaterThanOrEqual(500 - 64);
    expect(r.worstTile.x).toBeLessThan(620);
    expect(r.worstTile.y).toBeGreaterThanOrEqual(300 - 64);
    expect(r.worstTile.y).toBeLessThan(400);
  });

  it('un cambio pequeño queda ≥ 0.99 en total pero el peor cuadro lo delata', () => {
    const changed = withBlock(base, 40, 40, 30, 20, [230, 230, 235]);
    const r = compare(encode(changed), encode(base));
    expect(r.similarity).toBeGreaterThanOrEqual(0.99);
    expect(r.worstTile.similarity).toBeLessThan(0.9);
  });

  it('tolera diferencias de color mínimas (composición, redondeo)', () => {
    const jitter = image(1280, 860, (x, y) => {
      const [r, g, b] = screen(x, y);
      return [r + ((x + y) % 2), g, b];
    });
    expect(compare(encode(jitter), encode(base)).similarity).toBe(1);
  });

  it('el PNG de diferencias es válido y del mismo tamaño', () => {
    const changed = withBlock(base, 0, 0, 10, 10, [255, 255, 255]);
    const diff = PNG.sync.read(compare(encode(changed), encode(base)).diffPng);
    expect([diff.width, diff.height]).toEqual([1280, 860]);
  });

  it('tamaños distintos dan un error claro', () => {
    const small = image(100, 50, screen);
    expect(() => compare(encode(small), encode(base))).toThrow(SizeMismatchError);
    expect(() => compare(encode(small), encode(base))).toThrow(/100×50 contra 1280×860/);
  });

  it('propiedad: la similitud está en [0, 1], es simétrica y baja con más píxeles cambiados', () => {
    const next = rng(125);
    const small = image(96, 64, (x, y) => [(x * 3) % 256, (y * 5) % 256, 40]);
    for (let n = 0; n < 40; n++) {
      const w = 1 + Math.floor(next() * 40);
      const h = 1 + Math.floor(next() * 30);
      const x = Math.floor(next() * (96 - w));
      const y = Math.floor(next() * (64 - h));
      const rgb: Rgb = [Math.floor(next() * 256), Math.floor(next() * 256), Math.floor(next() * 256)];
      const a = withBlock(small, x, y, w, h, rgb);
      const bigger = withBlock(a, x, y, Math.min(w + 8, 96 - x), Math.min(h + 8, 64 - y), rgb);
      const ab = compare(encode(a), encode(small));
      const ba = compare(encode(small), encode(a));
      expect(ab.similarity).toBeGreaterThanOrEqual(0);
      expect(ab.similarity).toBeLessThanOrEqual(1);
      expect(ab.similarity).toBeCloseTo(ba.similarity, 10);
      expect(compare(encode(bigger), encode(small)).similarity).toBeLessThanOrEqual(ab.similarity);
    }
  });

  it('con una captura real de mockup: igual = 1.0, con un bloque < 0.99', () => {
    const file = join(SHOTS_DIR, 'V3-Puesto.png');
    if (!existsSync(file)) return; // se genera con `pnpm mockups:shot`
    const png = PNG.sync.read(readFileSync(file));
    expect(compare(file, file).similarity).toBe(1);
    const changed = withBlock(png, 600, 300, 120, 100, [255, 0, 255]);
    expect(compare(encode(changed), file).similarity).toBeLessThan(0.99);
  });
});
