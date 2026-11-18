import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PNG } from 'pngjs';
import { afterAll, describe, expect, it } from 'vitest';
import { MissingMockupShotError, compareWithMockup, mockupShotPath } from '../src/harness.ts';

// Fake references folder: it does not depend on docs/mockups-png being generated.
const shots = mkdtempSync(join(tmpdir(), 'mockup-harness-'));
afterAll(() => rmSync(shots, { recursive: true, force: true }));

function solid(width: number, height: number, rgb: [number, number, number], block?: [number, number, number, number]): Buffer {
  const png = new PNG({ width, height });
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const inBlock = block && x >= block[0] && x < block[0] + block[2] && y >= block[1] && y < block[1] + block[3];
      const i = (y * width + x) * 4;
      png.data[i] = inBlock ? 240 : rgb[0];
      png.data[i + 1] = inBlock ? 240 : rgb[1];
      png.data[i + 2] = inBlock ? 240 : rgb[2];
      png.data[i + 3] = 255;
    }
  }
  return PNG.sync.write(png);
}

const reference = solid(320, 200, [14, 15, 18]);
writeFileSync(join(shots, 'V3-Desk.png'), reference);
writeFileSync(join(shots, 'V3-Desk--scene-game.png'), solid(320, 200, [14, 15, 18], [10, 10, 40, 40]));

describe('export of @tandem/mockup-render/harness', () => {
  it('resolves by package name, as tandem-ui imports it', async () => {
    const harness = await import('@tandem/mockup-render/harness');
    expect(typeof harness.compareWithMockup).toBe('function');
    expect(typeof harness.capture).toBe('function');
    expect(typeof harness.launchBrowser).toBe('function');
    expect(harness.CHROMIUM_ARGS).toContain('--disable-partial-raster');
    const all = await import('@tandem/mockup-render');
    expect(all.compareWithMockup).toBe(harness.compareWithMockup);
  });
});

describe('compareWithMockup', () => {
  it('busca <mockup>.png o <mockup>--<estado>.png', () => {
    expect(mockupShotPath('V3-Desk', undefined, shots)).toBe(join(shots, 'V3-Desk.png'));
    expect(mockupShotPath('V3-Desk', 'scene-game', shots)).toBe(join(shots, 'V3-Desk--scene-game.png'));
  });

  it('the same screenshot gives similarity 1 and leaves the diff where requested', () => {
    const diffPath = join(shots, 'sub', 'dir', 'diff.png');
    const r = compareWithMockup(reference, { mockup: 'V3-Desk', shotsDir: shots, diffPath });
    expect(r.similarity).toBe(1);
    expect(r.worstTile.similarity).toBe(1);
    expect(r.expectedPath).toBe(join(shots, 'V3-Desk.png'));
    expect(r.diffPath).toBe(diffPath);
    const diff = PNG.sync.read(readFileSync(diffPath));
    expect([diff.width, diff.height]).toEqual([320, 200]);
  });

  it('against another state, the worst tile exposes the difference', () => {
    const file = join(shots, 'app.png');
    writeFileSync(file, reference);
    const r = compareWithMockup(file, { mockup: 'V3-Desk', state: 'scene-game', shotsDir: shots });
    expect(r.similarity).toBeGreaterThan(0.97);
    expect(r.worstTile.similarity).toBeLessThan(0.7);
    expect(r.diffPath).toBeUndefined();
  });

  it('accepts region and the compare options', () => {
    const r = compareWithMockup(reference, {
      mockup: 'V3-Desk',
      state: 'scene-game',
      shotsDir: shots,
      region: { x: 100, y: 0, width: 220, height: 200 },
    });
    expect(r.similarity).toBe(1);
  });

  it('without a reference it gives an error that says which one is missing', () => {
    expect(() => compareWithMockup(reference, { mockup: 'V3-Desk', state: 'does-not-exist', shotsDir: shots })).toThrow(
      MissingMockupShotError,
    );
    expect(() => mockupShotPath('V3-Nothing', undefined, shots)).toThrow(/V3-Nothing\.png.*pnpm mockups:shot/);
    expect(existsSync(join(shots, 'V3-Nothing.png'))).toBe(false);
  });
});
