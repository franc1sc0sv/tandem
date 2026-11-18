import { existsSync, readFileSync, statSync } from 'node:fs';
import { extname, join, normalize, sep } from 'node:path';
import { MOCKUP_SUFFIX, MOCKUPS_DIR, SUPPORT_JS, TOOL_DIR, listMockups } from './paths.ts';

export interface Asset {
  body: Buffer;
  contentType: string;
}

const TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.woff2': 'font/woff2',
  '.woff': 'font/woff',
};

function fileAsset(path: string): Asset {
  return { body: readFileSync(path), contentType: TYPES[extname(path)] ?? 'application/octet-stream' };
}

/** File inside `dir`, without escaping it (`..`). */
function inside(dir: string, pathname: string): string | null {
  const rel = normalize(decodeURIComponent(pathname)).replace(/^([/\\])+/, '');
  if (rel.startsWith('..')) return null;
  const path = join(dir, rel);
  if (!path.startsWith(dir.endsWith(sep) ? dir : dir + sep)) return null;
  return existsSync(path) && statSync(path).isFile() ? path : null;
}

function indexPage(dir: string): Asset {
  const items = listMockups(dir)
    .map((n) => `<li><a href="${n}${MOCKUP_SUFFIX}">${n}</a></li>`)
    .join('\n');
  const html = `<!doctype html><html lang="en"><meta charset="utf-8"><title>Mockups v0.3</title>
<body style="font-family:system-ui;background:#0e0f12;color:#e8e8eb"><h1>Mockups v0.3</h1><ul>${items}</ul></body></html>`;
  return { body: Buffer.from(html), contentType: TYPES['.html'] ?? 'text/html' };
}

/**
 * What the renderer serves for a path: our own `support.js` (any path that
 * ends in `/support.js`, because the mockups ask for it as `./support.js`), the files of
 * the mockups folder and an index at `/`.
 */
export function mockupAsset(pathname: string, dir: string = MOCKUPS_DIR): Asset | null {
  if (pathname === '/' || pathname === '') return indexPage(dir);
  if (pathname.endsWith('/support.js')) return fileAsset(SUPPORT_JS);
  const path = inside(dir, pathname);
  return path ? fileAsset(path) : null;
}

// ---- Fuentes -----------------------------------------------------------------------------
// The mockups request Inter and JetBrains Mono from Google Fonts. So screenshots do not depend
// on the network (or on what Google returns that day), the capturer answers that stylesheet with the
// mismas fuentes empaquetadas en `@fontsource/*`.

export const FONT_ORIGIN = 'http://fonts.local';

function fontPackageDir(family: string): string {
  return join(TOOL_DIR, 'node_modules', '@fontsource', family.trim().toLowerCase().replace(/\s+/g, '-'));
}

/**
 * Blocks that the Inter and JetBrains Mono subsets lack and the mockups use (⌃ ⌥ ⌘
 * in shortcuts, → ↵ ≈ ✓ ➜). Without this, each machine draws them with its system font and
 * the WSL and CI screenshots differ. They are covered with DejaVu Sans (complete) as extra faces
 * of the same family.
 */
export const FALLBACK_RANGES = 'U+2190-21FF, U+2200-22FF, U+2300-23FF, U+2700-27BF';

// One face per weight, with the exact same weight as the own faces: with a weight range
// (`550 900`) the fallback fits `font-weight:650` better than Inter's 700, and since it
// has no Latin, that text ends up in the system font.
function fallbackFaces(family: string, weights: string[]): string {
  return weights
    .map(
      (w) => `@font-face {
  font-family: '${family}';
  font-style: normal;
  font-display: swap;
  font-weight: ${w};
  src: url(${FONT_ORIGIN}/@fontsource/dejavu-sans/files/dejavu-sans-latin-${Number(w) >= 550 ? 700 : 400}-normal.woff2) format('woff2');
  unicode-range: ${FALLBACK_RANGES};
}`,
    )
    .join('\n');
}

/** CSS equivalent to `https://fonts.googleapis.com/css2?family=…` with the local fonts. */
export function googleFontsCss(url: URL): { css: string; missing: string[] } {
  // The fallback faces go first: with overlapping ranges the last defined wins, so
  // where the own font has the character, its own is used.
  const fallback: string[] = [];
  const css: string[] = [];
  const missing: string[] = [];
  for (const spec of url.searchParams.getAll('family')) {
    const [family = '', axes = ''] = spec.split(':');
    const weights = /wght@([\d;]+)/.exec(axes)?.[1]?.split(';') ?? ['400'];
    const dir = fontPackageDir(family);
    const pkg = dir.slice(dir.lastIndexOf('@fontsource'));
    fallback.push(fallbackFaces(family.replaceAll('+', ' ').trim(), weights));
    for (const w of weights) {
      const file = join(dir, `${w}.css`);
      if (!existsSync(file)) {
        missing.push(`${family} ${w}`);
        continue;
      }
      const text = readFileSync(file, 'utf8').replaceAll('url(./files/', `url(${FONT_ORIGIN}/${pkg.replaceAll(sep, '/')}/files/`);
      css.push(text);
    }
  }
  return { css: [...fallback, ...css].join('\n'), missing };
}

/** Font file requested from `FONT_ORIGIN` (`/@fontsource/<family>/files/<file>`). */
export function fontAsset(pathname: string): Asset | null {
  const path = inside(join(TOOL_DIR, 'node_modules'), pathname);
  return path && /\.woff2?$/.test(path) ? fileAsset(path) : null;
}
