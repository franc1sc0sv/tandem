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

/** Archivo dentro de `dir`, sin salirse de él (`..`). */
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
  const html = `<!doctype html><html lang="es"><meta charset="utf-8"><title>Mockups v0.3</title>
<body style="font-family:system-ui;background:#0e0f12;color:#e8e8eb"><h1>Mockups v0.3</h1><ul>${items}</ul></body></html>`;
  return { body: Buffer.from(html), contentType: TYPES['.html'] ?? 'text/html' };
}

/**
 * Lo que sirve el renderizador para una ruta: el `support.js` propio (cualquier ruta que
 * termine en `/support.js`, porque los mockups lo piden como `./support.js`), los archivos de
 * la carpeta de mockups y un índice en `/`.
 */
export function mockupAsset(pathname: string, dir: string = MOCKUPS_DIR): Asset | null {
  if (pathname === '/' || pathname === '') return indexPage(dir);
  if (pathname.endsWith('/support.js')) return fileAsset(SUPPORT_JS);
  const path = inside(dir, pathname);
  return path ? fileAsset(path) : null;
}

// ---- Fuentes -----------------------------------------------------------------------------
// Los mockups piden Inter y JetBrains Mono a Google Fonts. Para que las capturas no dependan
// de la red (ni de qué devuelva Google ese día), el capturador responde esa hoja con las
// mismas fuentes empaquetadas en `@fontsource/*`.

export const FONT_ORIGIN = 'http://fonts.local';

function fontPackageDir(family: string): string {
  return join(TOOL_DIR, 'node_modules', '@fontsource', family.trim().toLowerCase().replace(/\s+/g, '-'));
}

/**
 * Bloques que los subconjuntos de Inter y JetBrains Mono no traen y los mockups usan (⌃ ⌥ ⌘
 * en los atajos, → ↵ ≈ ✓ ➜). Sin esto, cada máquina los dibuja con su fuente del sistema y
 * las capturas de WSL y de CI difieren. Se cubren con DejaVu Sans (completa) como caras extra
 * de la misma familia.
 */
export const FALLBACK_RANGES = 'U+2190-21FF, U+2200-22FF, U+2300-23FF, U+2700-27BF';

// Una cara por peso, con el mismo peso exacto que las caras propias: con un rango de pesos
// (`550 900`) la de respaldo calza mejor con `font-weight:650` que la de 700 de Inter, y como
// no tiene latín, ese texto termina en la fuente del sistema.
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

/** CSS equivalente a `https://fonts.googleapis.com/css2?family=…` con las fuentes locales. */
export function googleFontsCss(url: URL): { css: string; missing: string[] } {
  // Las caras de respaldo van primero: con rangos superpuestos gana la última definida, así
  // que donde la fuente propia tiene el carácter se usa el suyo.
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

/** Archivo de fuente pedido a `FONT_ORIGIN` (`/@fontsource/<familia>/files/<archivo>`). */
export function fontAsset(pathname: string): Asset | null {
  const path = inside(join(TOOL_DIR, 'node_modules'), pathname);
  return path && /\.woff2?$/.test(path) ? fileAsset(path) : null;
}
