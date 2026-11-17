import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { PNG } from 'pngjs';
import { describe, expect, it } from 'vitest';
import { SizeMismatchError, compare, tileStarts, worstTileOf } from '../src/compare.ts';
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

describe('tileStarts / worstTileOf', () => {
  it('paso size/2 y un último cuadro pegado al borde', () => {
    expect(tileStarts(64, 64)).toEqual([0]);
    expect(tileStarts(40, 64)).toEqual([0]);
    expect(tileStarts(128, 64)).toEqual([0, 32, 64]);
    expect(tileStarts(150, 64)).toEqual([0, 32, 64, 86]);
    expect(tileStarts(10, 1)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  });

  it('propiedad: todo píxel queda dentro de algún cuadro y todos los cuadros caben', () => {
    const next = rng(64);
    for (let n = 0; n < 200; n++) {
      const size = 1 + Math.floor(next() * 80);
      const length = 1 + Math.floor(next() * 400);
      const starts = tileStarts(length, size);
      const w = Math.min(size, length);
      for (const s of starts) expect(s + w).toBeLessThanOrEqual(length);
      for (let p = 0; p < length; p++) expect(starts.some((s) => p >= s && p < s + w)).toBe(true);
    }
  });

  it('propiedad: es el mínimo exacto sobre la rejilla (contra la cuenta a mano)', () => {
    const next = rng(4);
    for (let n = 0; n < 60; n++) {
      const width = 1 + Math.floor(next() * 90);
      const height = 1 + Math.floor(next() * 70);
      const size = 1 + Math.floor(next() * 40);
      const density = next();
      const mask = Uint8Array.from({ length: width * height }, () => (next() < density * 0.3 ? 1 : 0));
      const w = Math.min(size, width);
      const h = Math.min(size, height);
      let expected = 1;
      for (const ty of tileStarts(height, size)) {
        for (const tx of tileStarts(width, size)) {
          let bad = 0;
          for (let y = ty; y < ty + h; y++) for (let x = tx; x < tx + w; x++) bad += mask[y * width + x] ?? 0;
          expected = Math.min(expected, 1 - bad / (w * h));
        }
      }
      const t = worstTileOf(mask, width, height, size);
      expect(t.similarity).toBeCloseTo(expected, 12);
      expect([t.width, t.height]).toEqual([w, h]);
      let bad = 0;
      for (let y = t.y; y < t.y + h; y++) for (let x = t.x; x < t.x + w; x++) bad += mask[y * width + x] ?? 0;
      expect(1 - bad / (w * h)).toBeCloseTo(t.similarity, 12);
    }
  });
});

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

  it('un cambio de 64×64 que cruza cuatro cuadros de la rejilla fija no se diluye', () => {
    // En (96, 96) cae en el medio de cuatro cuadros de 64 de una rejilla fija (25 % en cada
    // uno); la rejilla solapada tiene un cuadro justo encima.
    const changed = withBlock(base, 96, 96, 64, 64, [230, 230, 235]);
    const r = compare(encode(changed), encode(base));
    expect(r.worstTile).toEqual({ x: 96, y: 96, width: 64, height: 64, similarity: 0 });
  });

  it('el peor cuadro cubre también el borde derecho e inferior con cuadros enteros', () => {
    // 1280×860: 860 no es múltiplo de 64 ni de 32; el último cuadro empieza en 860 - 64.
    const changed = withBlock(base, 1250, 840, 30, 20, [230, 230, 235]);
    const t = compare(encode(changed), encode(base)).worstTile;
    expect([t.x, t.y, t.width, t.height]).toEqual([1216, 796, 64, 64]);
    expect(t.similarity).toBeCloseTo(1 - (30 * 20) / (64 * 64), 10);
  });

  it('con region compara solo esa zona y da el peor cuadro en coordenadas de la captura', () => {
    const changed = withBlock(base, 600, 400, 40, 40, [230, 230, 235]);
    const outside = compare(encode(changed), encode(base), { region: { x: 0, y: 0, width: 240, height: 860 } });
    expect(outside.similarity).toBe(1);
    expect([outside.width, outside.height]).toEqual([240, 860]);
    const inside = compare(encode(changed), encode(base), { region: { x: 560, y: 360, width: 200, height: 200 } });
    expect(inside.mismatched).toBeGreaterThan(0);
    expect(inside.total).toBe(200 * 200);
    expect(inside.worstTile.x).toBeGreaterThanOrEqual(560);
    expect(inside.worstTile.y).toBeGreaterThanOrEqual(360);
    expect(inside.worstTile.x).toBeLessThanOrEqual(600);
    expect(inside.worstTile.y).toBeLessThanOrEqual(400);
    const diff = PNG.sync.read(inside.diffPng);
    expect([diff.width, diff.height]).toEqual([200, 200]);
  });

  it('una region fuera de la imagen da un error claro', () => {
    expect(() => compare(encode(base), encode(base), { region: { x: 1200, y: 0, width: 100, height: 10 } })).toThrow(
      /región inválida: x=1200 y=0 100×10 en una captura de 1280×860/,
    );
    expect(() => compare(encode(base), encode(base), { region: { x: 0, y: 0, width: 0, height: 10 } })).toThrow(RangeError);
    expect(() => compare(encode(base), encode(base), { region: { x: 0.5, y: 0, width: 10, height: 10 } })).toThrow(RangeError);
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
