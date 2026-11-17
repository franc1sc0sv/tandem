import { describe, expect, it } from 'vitest';
import { FONT_ORIGIN, fontAsset, googleFontsCss, mockupAsset } from '../src/assets.ts';
import { listMockups, previewOf, shotName } from '../src/paths.ts';

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
    const url = new URL(
      'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap',
    );
    const { css, missing } = googleFontsCss(url);
    expect(missing).toEqual([]);
    expect(css).toContain("font-family: 'Inter'");
    expect(css).toContain("font-family: 'JetBrains Mono'");
    expect(css).not.toContain('url(./files/');
    const first = /url\((http:\/\/fonts\.local\/[^)]+\.woff2)\)/.exec(css)?.[1] ?? '';
    expect(first.startsWith(FONT_ORIGIN)).toBe(true);
    expect(fontAsset(new URL(first).pathname)?.contentType).toBe('font/woff2');
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
