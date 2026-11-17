import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { FALLBACK_RANGES, FONT_ORIGIN, fontAsset, googleFontsCss, mockupAsset } from '../src/assets.ts';
import { MOCKUPS_DIR, listMockups, previewOf, shotName } from '../src/paths.ts';

const GOOGLE_CSS = new URL(
  'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap',
);

function parseRanges(list: string): Array<[number, number]> {
  return list.split(',').map((part) => {
    const [a = '0', b] = part.trim().replace(/^U\+/i, '').split('-');
    return [parseInt(a, 16), parseInt(b ?? a, 16)];
  });
}

describe('assets', () => {
  it('sirve el support.js propio para cualquier ruta que lo pida', () => {
    const a = mockupAsset('/support.js');
    const b = mockupAsset('/sub/support.js');
    expect(a?.contentType).toMatch(/javascript/);
    expect(a?.body.toString()).toContain('DCLogic');
    expect(b?.body.equals(a?.body ?? Buffer.alloc(0))).toBe(true);
  });

  it('sirve los mockups y un índice con enlaces a todos', () => {
    expect(mockupAsset('/V3-Puesto.dc.html')?.body.toString()).toContain('<x-dc>');
    const index = mockupAsset('/')?.body.toString() ?? '';
    for (const name of listMockups()) expect(index).toContain(`href="${name}.dc.html"`);
  });

  it('no sale de la carpeta de mockups', () => {
    expect(mockupAsset('/../README.md')).toBeNull();
    expect(mockupAsset('/%2e%2e/README.md')).toBeNull();
    expect(mockupAsset('/../../Cargo.toml')).toBeNull();
    expect(mockupAsset('/no-existe.dc.html')).toBeNull();
  });

  it('cambia la hoja de Google Fonts por las fuentes empaquetadas', () => {
    const { css, missing } = googleFontsCss(GOOGLE_CSS);
    expect(missing).toEqual([]);
    expect(css).toContain("font-family: 'Inter'");
    expect(css).toContain("font-family: 'JetBrains Mono'");
    expect(css).not.toContain('url(./files/');
    const first = /url\((http:\/\/fonts\.local\/[^)]+\.woff2)\)/.exec(css)?.[1] ?? '';
    expect(first.startsWith(FONT_ORIGIN)).toBe(true);
    expect(fontAsset(new URL(first).pathname)?.contentType).toBe('font/woff2');
  });

  it('cubre con fuentes empaquetadas cada carácter no ASCII de los mockups', () => {
    // Si no, ese carácter sale con la fuente del sistema y la captura cambia según la máquina.
    const { css } = googleFontsCss(GOOGLE_CSS);
    const ranges = [...css.matchAll(/unicode-range:\s*([^;]+);/g)].flatMap((m) => parseRanges(m[1] ?? ''));
    const uncovered = new Set<string>();
    for (const name of listMockups()) {
      const html = readFileSync(join(MOCKUPS_DIR, name + '.dc.html'), 'utf8').replace(/<(style|script)[\s\S]*?<\/\1>/g, '');
      const text = html.replace(/<[^>]*>/g, '');
      for (const ch of text) {
        const cp = ch.codePointAt(0) ?? 0;
        if (cp >= 128 && !ranges.some(([a, b]) => cp >= a && cp <= b)) uncovered.add(`${ch} U+${cp.toString(16)} (${name})`);
      }
    }
    expect([...uncovered]).toEqual([]);
  });

  it('las caras de respaldo van antes que las propias (gana la última en los rangos superpuestos)', () => {
    const { css } = googleFontsCss(GOOGLE_CSS);
    expect(css.indexOf(FALLBACK_RANGES)).toBeGreaterThanOrEqual(0);
    expect(css.lastIndexOf(FALLBACK_RANGES)).toBeLessThan(css.indexOf('inter-latin-400-normal'));
    expect(css).toContain("font-family: 'JetBrains Mono';\n  font-style: normal;\n  font-display: swap;\n  font-weight: 400;");
    const dejavu = /url\((http:\/\/fonts\.local\/@fontsource\/dejavu-sans\/[^)]+)\)/.exec(css)?.[1] ?? '';
    expect(fontAsset(new URL(dejavu).pathname)?.contentType).toBe('font/woff2');
  });

  it('informa las variantes que no están empaquetadas', () => {
    const { missing } = googleFontsCss(new URL('https://fonts.googleapis.com/css2?family=Roboto:wght@400'));
    expect(missing).toEqual(['Roboto 400']);
  });

  it('solo sirve archivos de fuentes desde node_modules', () => {
    expect(fontAsset('/playwright/package.json')).toBeNull();
    expect(fontAsset('/../package.json')).toBeNull();
  });
});

describe('paths', () => {
  it('lee el tamaño de $preview o usa 1280×860', () => {
    expect(previewOf(`<script data-props='{"$preview":{"width":380,"height":600}}'>`)).toEqual({ width: 380, height: 600 });
    expect(previewOf('<script>')).toEqual({ width: 1280, height: 860 });
    expect(previewOf(`<script data-props='no es json'>`)).toEqual({ width: 1280, height: 860 });
  });

  it('nombra las capturas por mockup y estado', () => {
    expect(shotName('V3-Acomodo')).toBe('V3-Acomodo.png');
    expect(shotName('V3-Acomodo', 'lado-arriba-fin')).toBe('V3-Acomodo--lado-arriba-fin.png');
  });
});
