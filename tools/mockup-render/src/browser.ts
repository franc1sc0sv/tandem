import type { Browser, BrowserContext, LaunchOptions, Page } from 'playwright';
import { FONT_ORIGIN, fontAsset, googleFontsCss, mockupAsset } from './assets.ts';
import { MOCKUPS_DIR } from './paths.ts';

/** Origen ficticio desde el que se sirven los mockups dentro del navegador. */
export const ORIGIN = 'http://mockups.local';

/**
 * Argumentos de Chromium para que la misma página dé siempre el mismo PNG.
 *
 * Con «partial raster» (lo normal), Chromium vuelve a rasterizar solo la parte invalidada de
 * cada baldosa y deja el resto como estaba. El resultado final depende entonces de qué cuadros
 * intermedios llegó a pintar durante una transición: en `V3-Acomodo` la pantalla con
 * `opacity: .4` salía con ±1 de color en toda su área según la corrida (hasta ~10 600 píxeles
 * distintos). Rasterizando la baldosa entera cada vez, el PNG ya no depende del historial.
 */
export const CHROMIUM_ARGS: readonly string[] = ['--disable-partial-raster'];

/**
 * Abre Chromium headless con `CHROMIUM_ARGS`. El arnés de la app (T-112) tiene que lanzar el
 * navegador así para que sus capturas se comparen en igualdad de condiciones con las de los
 * mockups.
 */
export async function launchBrowser(opts: LaunchOptions = {}): Promise<Browser> {
  const { chromium } = await import('playwright');
  return chromium.launch({ ...opts, args: [...CHROMIUM_ARGS, ...(opts.args ?? [])] });
}

/**
 * Captura la página como se capturan los mockups: espera a que quede quieta (`settle`) y saca
 * la captura con las animaciones infinitas quitadas (quedan con su estilo base) y sin el cursor
 * de texto. Devuelve el PNG y, si se pasa `path`, también lo guarda.
 */
export async function capture(page: Page, path?: string): Promise<Buffer> {
  await settle(page);
  return page.screenshot({ path, animations: 'disabled', caret: 'hide' });
}

export interface RouteOptions {
  /** Carpeta de mockups (por omisión `mockups/v03`). */
  dir?: string;
  /** Páginas extra por ruta (`/x.dc.html` → HTML), para las pruebas del runtime. */
  pages?: Record<string, string>;
  /** Se llama con cada petición bloqueada (todo lo externo que no sean las fuentes). */
  onBlocked?: (url: string) => void;
}

/**
 * Sirve los mockups, `support.js` y las fuentes desde Node dentro del contexto, sin servidor
 * ni red: cualquier otra petición externa se bloquea (y se informa), así la captura es
 * siempre la misma.
 */
export async function installRoutes(context: BrowserContext, opts: RouteOptions = {}): Promise<void> {
  const dir = opts.dir ?? MOCKUPS_DIR;
  await context.route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (url.origin === ORIGIN) {
      const page = opts.pages?.[url.pathname];
      if (page !== undefined) return route.fulfill({ body: page, contentType: 'text/html; charset=utf-8' });
      const asset = mockupAsset(url.pathname, dir);
      if (asset) return route.fulfill({ body: asset.body, contentType: asset.contentType });
      return route.fulfill({ status: 404, body: 'no existe: ' + url.pathname });
    }
    if (url.hostname === 'fonts.googleapis.com') {
      const { css, missing } = googleFontsCss(url);
      for (const m of missing) opts.onBlocked?.(`fuente sin empaquetar: ${m}`);
      return route.fulfill({ body: css, contentType: 'text/css; charset=utf-8' });
    }
    if (url.origin === FONT_ORIGIN) {
      const asset = fontAsset(url.pathname);
      if (asset) return route.fulfill({ body: asset.body, contentType: asset.contentType });
    }
    opts.onBlocked?.(url.href);
    return route.abort('blockedbyclient');
  });
}

/** Espera a que `support.js` haya hecho el primer render. */
export async function waitForMockup(page: Page, timeoutMs = 15_000): Promise<void> {
  await page.waitForFunction(() => (window as { __dc?: { ready?: boolean } }).__dc?.ready === true, undefined, {
    timeout: timeoutMs,
  });
}

/**
 * Espera a que la página quede quieta: fuentes cargadas y todas las animaciones y transiciones
 * finitas terminadas (las de entrada, las de un clic). Las infinitas (pulsos, brillos) no
 * terminan nunca: para la captura se usa `animations: 'disabled'`, que las quita y deja el
 * estilo base del elemento. Sirve igual para la app (T-112) que para los mockups.
 */
export async function settle(page: Page, timeoutMs = 6_000): Promise<void> {
  await page.evaluate(async (timeout) => {
    const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    await frame();
    await frame();
    await document.fonts.ready;
    const deadline = performance.now() + timeout;
    for (;;) {
      const running = document.getAnimations().filter((a) => {
        const end = a.effect?.getComputedTiming().endTime;
        return a.playState !== 'finished' && a.playState !== 'idle' && a.playState !== 'paused' && end !== Infinity;
      });
      const left = deadline - performance.now();
      if (running.length === 0 || left <= 0) break;
      await Promise.race([
        Promise.all(running.map((a) => a.finished.catch(() => undefined))),
        new Promise((resolve) => setTimeout(resolve, left)),
      ]);
      await frame();
    }
    await document.fonts.ready;
  }, timeoutMs);
}

export interface PageCheck {
  /** Textos visibles o atributos con `{{` sin sustituir. */
  braces: string[];
  /** Errores que informó `support.js` (expresiones que fallan, componente que no arranca). */
  dcErrors: string[];
  /** Expresiones que dieron `undefined`. */
  dcMissing: string[];
  /** Fuentes que fallaron, o Inter sin cargar en una página que la pide. */
  fontErrors: string[];
}

export async function checkPage(page: Page): Promise<PageCheck> {
  return page.evaluate(() => {
    const braces: string[] = [];
    if (document.body.innerText.includes('{{')) braces.push('texto: ' + document.body.innerText.slice(0, 200));
    for (const el of Array.from(document.body.querySelectorAll('*'))) {
      for (const a of Array.from(el.attributes)) {
        if (a.value.includes('{{')) braces.push(`<${el.localName} ${a.name}="${a.value}">`);
      }
    }
    const faces = Array.from(document.fonts);
    const fontErrors = faces.filter((f) => f.status === 'error').map((f) => `${f.family} ${f.weight}: no cargó`);
    const wantsInter = !!document.querySelector('link[href*="fonts.googleapis.com/css2"][href*="Inter"]');
    if (wantsInter && !faces.some((f) => f.family.replace(/["']/g, '') === 'Inter' && f.status === 'loaded')) {
      fontErrors.push('Inter: ninguna variante cargada');
    }
    const dc = (window as { __dc?: { errors: string[]; missing: string[] } }).__dc;
    return {
      braces,
      dcErrors: dc ? [...dc.errors] : ['sin window.__dc'],
      dcMissing: dc ? [...dc.missing] : [],
      fontErrors,
    };
  });
}
