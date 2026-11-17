// Todos los mockups de mockups/v03 (y sus estados con nombre) renderizan sin errores de
// consola, sin `{{` visibles, sin valores ausentes y con las fuentes cargadas.
import { existsSync, readdirSync } from 'node:fs';
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

  it('docs/mockups-png tiene exactamente una captura por mockup y estado', () => {
    if (!existsSync(SHOTS_DIR)) return; // aún sin generar
    const expected = jobsFor(mockups).map((j) => shotName(j.mockup, j.state)).sort();
    const actual = readdirSync(SHOTS_DIR).filter((f) => f.endsWith('.png')).sort();
    // Si falla: `pnpm mockups:shot` y commitear docs/mockups-png.
    expect(actual).toEqual(expected);
  });
});
