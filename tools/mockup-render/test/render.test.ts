import { describe, expect, it } from 'vitest';
import { jobsFor, parseConcurrency, renderMockups, staleShots } from '../src/render.ts';

describe('parseConcurrency', () => {
  it('acepta enteros ≥ 1', () => {
    expect(parseConcurrency('4')).toBe(4);
    expect(parseConcurrency(' 2 ')).toBe(2);
    expect(parseConcurrency(1)).toBe(1);
  });

  it('rechaza lo demás con un error que dice qué llegó', () => {
    for (const bad of ['abc', '', '0', '-1', '1.5', 'NaN', 'Infinity']) {
      expect(() => parseConcurrency(bad)).toThrow(RangeError);
    }
    expect(() => parseConcurrency(Number.NaN)).toThrow(RangeError);
    expect(() => parseConcurrency('abc')).toThrow('concurrency tiene que ser un entero ≥ 1 (llegó "abc")');
  });

  it('renderMockups no arranca con una concurrencia inválida (antes quedaban huecos en los resultados)', async () => {
    await expect(renderMockups({ names: ['V3-Puesto'], concurrency: Number.NaN })).rejects.toThrow(RangeError);
    await expect(renderMockups({ names: ['V3-Puesto'], concurrency: 0 })).rejects.toThrow(RangeError);
  });
});

describe('staleShots', () => {
  const jobs = jobsFor(['V3-Acomodo', 'V3-Puesto']);

  it('marca solo las capturas sin mockup o estado', () => {
    const existing = [
      'V3-Puesto.png',
      'V3-Acomodo.png',
      'V3-Acomodo--lado-arriba-centro.png',
      'V3-Acomodo--estado-viejo.png',
      'V3-Borrado.png',
      'notas.txt',
    ];
    expect(staleShots(existing, jobs)).toEqual(['V3-Acomodo--estado-viejo.png', 'V3-Borrado.png']);
  });

  it('no depende de los resultados: un trabajo que falló sigue protegiendo su referencia', () => {
    // Antes se armaba la lista desde `r.file`, que queda vacío si el trabajo revienta antes de
    // capturar, y la corrida completa borraba esa referencia.
    const existing = jobs.map((j) => (j.state ? `${j.mockup}--${j.state}.png` : `${j.mockup}.png`));
    expect(staleShots(existing, jobs)).toEqual([]);
  });
});
