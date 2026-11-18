import { describe, expect, it } from 'vitest';
import { jobsFor, parseConcurrency, renderMockups, staleShots } from '../src/render.ts';

describe('parseConcurrency', () => {
  it('accepts integers ≥ 1', () => {
    expect(parseConcurrency('4')).toBe(4);
    expect(parseConcurrency(' 2 ')).toBe(2);
    expect(parseConcurrency(1)).toBe(1);
  });

  it('rejects anything else with an error that says what arrived', () => {
    for (const bad of ['abc', '', '0', '-1', '1.5', 'NaN', 'Infinity']) {
      expect(() => parseConcurrency(bad)).toThrow(RangeError);
    }
    expect(() => parseConcurrency(Number.NaN)).toThrow(RangeError);
    expect(() => parseConcurrency('abc')).toThrow('concurrency must be an integer ≥ 1 (got "abc")');
  });

  it('renderMockups does not start with an invalid concurrency (it used to leave holes in the results)', async () => {
    await expect(renderMockups({ names: ['V3-Desk'], concurrency: Number.NaN })).rejects.toThrow(RangeError);
    await expect(renderMockups({ names: ['V3-Desk'], concurrency: 0 })).rejects.toThrow(RangeError);
  });
});

describe('staleShots', () => {
  const jobs = jobsFor(['V3-Arrangement', 'V3-Desk']);

  it('flags only the screenshots without a mockup or state', () => {
    const existing = [
      'V3-Desk.png',
      'V3-Arrangement.png',
      'V3-Arrangement--side-top-center.png',
      'V3-Arrangement--old-state.png',
      'V3-Deleted.png',
      'notas.txt',
    ];
    expect(staleShots(existing, jobs)).toEqual(['V3-Arrangement--old-state.png', 'V3-Deleted.png']);
  });

  it('does not depend on the results: a job that failed keeps protecting its reference', () => {
    // The list used to be built from `r.file`, which stays empty if the job blows up before
    // capturing, and the full run deleted that reference.
    const existing = jobs.map((j) => (j.state ? `${j.mockup}--${j.state}.png` : `${j.mockup}.png`));
    expect(staleShots(existing, jobs)).toEqual([]);
  });
});
