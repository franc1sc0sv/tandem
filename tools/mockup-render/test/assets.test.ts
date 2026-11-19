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

// What can end up on screen: the HTML text (without tags), the CSS `content:` values and
// the component code (`<script type="text/x-dc">`), where the texts of
// `renderVals()` come from. The other `<script>` tags (`support.js`) have no text of their own.
function visibleSource(html: string): string {
  const extra: string[] = [];
  const rest = html
    .replace(/<style\b[^>]*>([\s\S]*?)<\/style>/g, (_all, css: string) => {
      for (const m of css.matchAll(/content:\s*(["'])(.*?)\1/g)) extra.push(m[2] ?? '');
      return '';
    })
    .replace(/<script\b([^>]*)>([\s\S]*?)<\/script>/g, (_all, attrs: string, body: string) => {
      if (/type=["']text\/x-dc["']/.test(attrs)) extra.push(body);
      return '';
    });
  return [rest.replace(/<[^>]*>/g, ''), ...extra].join('\n');
}

describe('assets', () => {
  it('visibleSource includes the component and the CSS content: values, not the tags', () => {
    const html =
      '<style>.a{color:#fff}.a::after{content:"≠"}</style><p title="✗">hola ✓</p><script src="./support.js"></script>' +
      '<script type="text/x-dc" data-dc-script>class Component { renderVals() { return { k: "⌘" }; } }</script>';
    const text = visibleSource(html);
    expect(text).toContain('hola ✓');
    expect(text).toContain('⌘');
    expect(text).toContain('≠');
    expect(text).not.toContain('color:#fff');
    expect(text).not.toContain('✗');
  });

  it('serves our own support.js for any path that asks for it', () => {
    const a = mockupAsset('/support.js');
    const b = mockupAsset('/sub/support.js');
    expect(a?.contentType).toMatch(/javascript/);
    expect(a?.body.toString()).toContain('DCLogic');
    expect(b?.body.equals(a?.body ?? Buffer.alloc(0))).toBe(true);
  });

  it('serves the mockups and an index linking to all of them', () => {
    expect(mockupAsset('/V3-Desk.dc.html')?.body.toString()).toContain('<x-dc>');
    const index = mockupAsset('/')?.body.toString() ?? '';
    for (const name of listMockups()) expect(index).toContain(`href="${name}.dc.html"`);
  });

  it('does not leave the mockups folder', () => {
    expect(mockupAsset('/../README.md')).toBeNull();
    expect(mockupAsset('/%2e%2e/README.md')).toBeNull();
    expect(mockupAsset('/../../Cargo.toml')).toBeNull();
    expect(mockupAsset('/does-not-exist.dc.html')).toBeNull();
  });

  it('replaces the Google Fonts stylesheet with the bundled fonts', () => {
    const { css, missing } = googleFontsCss(GOOGLE_CSS);
    expect(missing).toEqual([]);
    expect(css).toContain("font-family: 'Inter'");
    expect(css).toContain("font-family: 'JetBrains Mono'");
    expect(css).not.toContain('url(./files/');
    const first = /url\((http:\/\/fonts\.local\/[^)]+\.woff2)\)/.exec(css)?.[1] ?? '';
    expect(first.startsWith(FONT_ORIGIN)).toBe(true);
    expect(fontAsset(new URL(first).pathname)?.contentType).toBe('font/woff2');
  });

  it('covers every non-ASCII character of the mockups with bundled fonts', () => {
    // Otherwise that character comes out in the system font and the screenshot changes per machine.
    const { css } = googleFontsCss(GOOGLE_CSS);
    const ranges = [...css.matchAll(/unicode-range:\s*([^;]+);/g)].flatMap((m) => parseRanges(m[1] ?? ''));
    const uncovered = new Set<string>();
    const texts = new Map<string, string>();
    for (const name of listMockups()) {
      texts.set(name, visibleSource(readFileSync(join(MOCKUPS_DIR, name + '.dc.html'), 'utf8')));
    }
    // The `renderVals()` texts are visible too (⌃⌥ of F-Palette, → of V3-Compute): the test
    // has to read the component, not just the HTML.
    expect(texts.get('F-Palette')).toContain('⌃');
    expect(texts.get('V3-Compute')).toContain('→');
    for (const [name, text] of texts) {
      for (const ch of text) {
        const cp = ch.codePointAt(0) ?? 0;
        if (cp >= 128 && !ranges.some(([a, b]) => cp >= a && cp <= b)) uncovered.add(`${ch} U+${cp.toString(16)} (${name})`);
      }
    }
    expect([...uncovered]).toEqual([]);
  });

  it('fallback faces come before the own ones (the last wins in overlapping ranges)', () => {
    const { css } = googleFontsCss(GOOGLE_CSS);
    expect(css.indexOf(FALLBACK_RANGES)).toBeGreaterThanOrEqual(0);
    expect(css.lastIndexOf(FALLBACK_RANGES)).toBeLessThan(css.indexOf('inter-latin-400-normal'));
    expect(css).toContain("font-family: 'JetBrains Mono';\n  font-style: normal;\n  font-display: swap;\n  font-weight: 400;");
    const dejavu = /url\((http:\/\/fonts\.local\/@fontsource\/dejavu-sans\/[^)]+)\)/.exec(css)?.[1] ?? '';
    expect(fontAsset(new URL(dejavu).pathname)?.contentType).toBe('font/woff2');
  });

  it('reports the variants that are not bundled', () => {
    const { missing } = googleFontsCss(new URL('https://fonts.googleapis.com/css2?family=Roboto:wght@400'));
    expect(missing).toEqual(['Roboto 400']);
  });

  it('only serves font files from node_modules', () => {
    expect(fontAsset('/playwright/package.json')).toBeNull();
    expect(fontAsset('/../package.json')).toBeNull();
  });
});

describe('paths', () => {
  it('reads the $preview size or uses 1280×860', () => {
    expect(previewOf(`<script data-props='{"$preview":{"width":380,"height":600}}'>`)).toEqual({ width: 380, height: 600 });
    expect(previewOf('<script>')).toEqual({ width: 1280, height: 860 });
    expect(previewOf(`<script data-props='not json'>`)).toEqual({ width: 1280, height: 860 });
  });

  it('names screenshots by mockup and state', () => {
    expect(shotName('V3-Arrangement')).toBe('V3-Arrangement.png');
    expect(shotName('V3-Arrangement', 'side-top-end')).toBe('V3-Arrangement--side-top-end.png');
  });
});
