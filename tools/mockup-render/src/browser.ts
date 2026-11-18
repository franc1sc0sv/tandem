import type { Browser, BrowserContext, LaunchOptions, Page } from 'playwright';
import { FONT_ORIGIN, fontAsset, googleFontsCss, mockupAsset } from './assets.ts';
import { MOCKUPS_DIR } from './paths.ts';

/** Fake origin the mockups are served from inside the browser. */
export const ORIGIN = 'http://mockups.local';

/**
 * Chromium arguments so the same page always yields the same PNG.
 *
 * With “partial raster” (the default), Chromium re-rasterizes only the invalidated part of
 * each tile and leaves the rest as it was. The final result then depends on which intermediate
 * frames it got to paint during a transition: in `V3-Arrangement` the screen with
 * `opacity: .4` came out with ±1 of color over its whole area depending on the run (up to ~10,600 pixels
 * different). Rasterizing the whole tile every time, the PNG no longer depends on history.
 */
export const CHROMIUM_ARGS: readonly string[] = ['--disable-partial-raster'];

/**
 * Opens headless Chromium with `CHROMIUM_ARGS`. The app harness (T-112) has to launch the
 * browser this way so its screenshots are compared on equal terms with those of the
 * mockups.
 */
export async function launchBrowser(opts: LaunchOptions = {}): Promise<Browser> {
  const { chromium } = await import('playwright');
  return chromium.launch({ ...opts, args: [...CHROMIUM_ARGS, ...(opts.args ?? [])] });
}

/**
 * Captures the page the way mockups are captured: waits for it to settle (`settle`) and takes
 * the screenshot with infinite animations removed (they keep their base style) and without the text
 * caret. Returns the PNG and, if `path` is given, saves it too.
 */
export async function capture(page: Page, path?: string): Promise<Buffer> {
  await settle(page);
  return page.screenshot({ path, animations: 'disabled', caret: 'hide' });
}

export interface RouteOptions {
  /** Mockups folder (default `mockups/v03`). */
  dir?: string;
  /** Extra pages by path (`/x.dc.html` → HTML), for the runtime tests. */
  pages?: Record<string, string>;
  /** Called with each blocked request (everything external that is not the fonts). */
  onBlocked?: (url: string) => void;
}

/**
 * Serves the mockups, `support.js` and the fonts from Node inside the context, with no server
 * or network: any other external request is blocked (and reported), so the screenshot is
 * always the same.
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
      for (const m of missing) opts.onBlocked?.(`unbundled font: ${m}`);
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

/** Waits for `support.js` to have done the first render. */
export async function waitForMockup(page: Page, timeoutMs = 15_000): Promise<void> {
  await page.waitForFunction(() => (window as { __dc?: { ready?: boolean } }).__dc?.ready === true, undefined, {
    timeout: timeoutMs,
  });
}

/**
 * Waits for the page to settle: fonts loaded and every finite animation and transition
 * finished (entrance ones, click ones). Infinite ones (pulses, glows) never
 * finish: for the screenshot `animations: 'disabled'` is used, which removes them and leaves the
 * element's base style. It works the same for the app (T-112) as for the mockups.
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
  /** Visible texts or attributes with an unsubstituted `{{`. */
  braces: string[];
  /** Errors reported by `support.js` (failing expressions, component that does not start). */
  dcErrors: string[];
  /** Expressions that evaluated to `undefined`. */
  dcMissing: string[];
  /** Fonts that failed, or Inter not loaded on a page that asks for it. */
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
    const fontErrors = faces.filter((f) => f.status === 'error').map((f) => `${f.family} ${f.weight}: failed to load`);
    const wantsInter = !!document.querySelector('link[href*="fonts.googleapis.com/css2"][href*="Inter"]');
    if (wantsInter && !faces.some((f) => f.family.replace(/["']/g, '') === 'Inter' && f.status === 'loaded')) {
      fontErrors.push('Inter: ninguna variante cargada');
    }
    const dc = (window as { __dc?: { errors: string[]; missing: string[] } }).__dc;
    return {
      braces,
      dcErrors: dc ? [...dc.errors] : ['no window.__dc'],
      dcMissing: dc ? [...dc.missing] : [],
      fontErrors,
    };
  });
}
