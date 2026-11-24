// All the mockups in mockups/v03 (and their named states) render without console
// errors, with no visible `{{`, no missing values and with the fonts loaded.
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { SHOTS_DIR, listMockups, shotName } from '../src/paths.ts';
import { jobsFor, problems, renderMockups } from '../src/render.ts';
import { STATES } from '../src/states.ts';

const mockups = listMockups();

describe('mockups v03', () => {
  it('are the 23 of canvas.json', () => {
    expect(mockups).toHaveLength(23);
  });

  it('states point to mockups that exist and their names are unique and kebab-case', () => {
    for (const [mockup, states] of Object.entries(STATES)) {
      expect(mockups).toContain(mockup);
      const names = states.map((s) => s.name);
      expect(new Set(names).size).toBe(names.length);
      for (const n of names) expect(n).toMatch(/^[a-z0-9]+(-[a-z0-9]+)*$/);
      for (const s of states) expect(s.steps.length).toBeGreaterThan(0);
    }
  });

  it('V3-Arrangement has the 4 sides × 3 alignments', () => {
    const names = (STATES['V3-Arrangement'] ?? []).map((s) => s.name);
    expect(names.filter((n) => n.startsWith('side-'))).toHaveLength(12);
  });

  it('all render cleanly, also in each state', async () => {
    const results = await renderMockups();
    expect(results).toHaveLength(jobsFor(mockups).length);
    const failures = results
      .map((r) => ({ shot: shotName(r.mockup, r.state), issues: problems(r) }))
      .filter((f) => f.issues.length > 0);
    expect(failures).toEqual([]);
  });

  it('the same screenshot comes out bit-identical across different runs', async () => {
    // The V3-Arrangement states with the most differences before --disable-partial-raster (up to
    // ~10,600 pixels in side-top-center, ±1 of color on the screen with opacity .4).
    const names = ['side-top-center', 'side-left-center', 'screen-mac'];
    const dirs = [0, 1, 2].map(() => mkdtempSync(join(tmpdir(), 'mockup-det-')));
    try {
      for (const outDir of dirs) {
        const results = await renderMockups({ names: ['V3-Arrangement'], outDir, concurrency: 4 });
        expect(results.flatMap(problems)).toEqual([]);
      }
      for (const state of names) {
        const [first, ...rest] = dirs.map((d) => readFileSync(join(d, shotName('V3-Arrangement', state))));
        for (const other of rest) expect(other.equals(first ?? Buffer.alloc(0)), state).toBe(true);
      }
    } finally {
      for (const d of dirs) rmSync(d, { recursive: true, force: true });
    }
  });

  it('docs/mockups-png has exactly one screenshot per mockup and state', () => {
    if (!existsSync(SHOTS_DIR)) return; // not generated yet
    const expected = jobsFor(mockups).map((j) => shotName(j.mockup, j.state)).sort();
    const actual = readdirSync(SHOTS_DIR).filter((f) => f.endsWith('.png')).sort();
    // If it fails: run `pnpm mockups:shot` and commit docs/mockups-png.
    expect(actual).toEqual(expected);
  });
});
