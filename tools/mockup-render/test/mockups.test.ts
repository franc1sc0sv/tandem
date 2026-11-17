// Todos los mockups de mockups/v03 (y sus estados con nombre) renderizan sin errores de
// consola, sin `{{` visibles, sin valores ausentes y con las fuentes cargadas.
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { SHOTS_DIR, listMockups, shotName } from '../src/paths.ts';
import { jobsFor, problems, renderMockups } from '../src/render.ts';
import { STATES } from '../src/states.ts';

const mockups = listMockups();

describe('mockups v03', () => {
  it('son los 23 de canvas.json', () => {
    expect(mockups).toHaveLength(23);
  });

  it('los estados apuntan a mockups que existen y sus nombres son únicos y en kebab-case', () => {
    for (const [mockup, states] of Object.entries(STATES)) {
      expect(mockups).toContain(mockup);
      const names = states.map((s) => s.name);
      expect(new Set(names).size).toBe(names.length);
      for (const n of names) expect(n).toMatch(/^[a-z0-9]+(-[a-z0-9]+)*$/);
      for (const s of states) expect(s.steps.length).toBeGreaterThan(0);
    }
  });

  it('V3-Acomodo tiene los 4 lados × 3 alineaciones', () => {
    const names = (STATES['V3-Acomodo'] ?? []).map((s) => s.name);
    expect(names.filter((n) => n.startsWith('lado-'))).toHaveLength(12);
  });

  it('todos renderizan limpios, también en cada estado', async () => {
    const results = await renderMockups();
    expect(results).toHaveLength(jobsFor(mockups).length);
    const failures = results
      .map((r) => ({ shot: shotName(r.mockup, r.state), issues: problems(r) }))
      .filter((f) => f.issues.length > 0);
    expect(failures).toEqual([]);
  });

  it('la misma captura sale idéntica bit a bit en corridas distintas', async () => {
    // Los estados de V3-Acomodo con más diferencias antes de --disable-partial-raster (hasta
    // ~10 600 píxeles en lado-arriba-centro, ±1 de color en la pantalla con opacity .4).
    const names = ['lado-arriba-centro', 'lado-izquierda-centro', 'pantalla-mac'];
    const dirs = [0, 1, 2].map(() => mkdtempSync(join(tmpdir(), 'mockup-det-')));
    try {
      for (const outDir of dirs) {
        const results = await renderMockups({ names: ['V3-Acomodo'], outDir, concurrency: 4 });
        expect(results.flatMap(problems)).toEqual([]);
      }
      for (const state of names) {
        const [first, ...rest] = dirs.map((d) => readFileSync(join(d, shotName('V3-Acomodo', state))));
        for (const other of rest) expect(other.equals(first ?? Buffer.alloc(0)), state).toBe(true);
      }
    } finally {
      for (const d of dirs) rmSync(d, { recursive: true, force: true });
    }
  });

  it('docs/mockups-png tiene exactamente una captura por mockup y estado', () => {
    if (!existsSync(SHOTS_DIR)) return; // aún sin generar
    const expected = jobsFor(mockups).map((j) => shotName(j.mockup, j.state)).sort();
    const actual = readdirSync(SHOTS_DIR).filter((f) => f.endsWith('.png')).sort();
    // Si falla: `pnpm mockups:shot` y commitear docs/mockups-png.
    expect(actual).toEqual(expected);
  });
});
