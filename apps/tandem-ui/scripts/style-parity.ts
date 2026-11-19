// Style parity (T-127), the browser part: reads the computed styles of the gallery
// and of the mockups and compares them with `style-parity-spec.ts`. `pnpm shots` uses it.

import { ORIGIN, installRoutes, statesOf, waitForMockup } from "@tandem/mockup-render";
import { settle } from "@tandem/mockup-render/harness";
import type { Browser, Page } from "playwright";
import { PAIRS, compareStyles, propertiesOf, type StyleDiff, type StylePair, type Styles, type Target } from "./style-parity-spec.ts";

interface Wanted {
  key: string;
  target: Target;
  props: string[];
}

/** Computed styles of each target (or `null` if the selector finds nothing). */
async function read(page: Page, wanted: Wanted[]): Promise<Record<string, Styles | null>> {
  return page.evaluate((list) => {
    const out: Record<string, Record<string, string> | null> = {};
    for (const w of list) {
      const el = document.querySelector(w.target.selector);
      if (!el) {
        out[w.key] = null;
        continue;
      }
      const cs = getComputedStyle(el, w.target.pseudo ?? null);
      out[w.key] = Object.fromEntries(w.props.map((p) => [p, cs.getPropertyValue(p)]));
    }
    return out;
  }, wanted);
}

const VIEWPORT = { width: 1280, height: 860 };

export interface ParityResult {
  pairs: StylePair[];
  diffs: StyleDiff[];
  /** Selectors that did not find their element, as `"pair": where`. */
  missing: string[];
  /** Gallery capture. */
  galleryPng: Buffer;
}

/**
 * Opens the gallery with `openGallery` (which serves the app and navigates) and each mockup of `pairs`, reads
 * the computed styles of both sides and compares them.
 */
export async function runStyleParity(browser: Browser, openGallery: (page: Page) => Promise<void>, pairs: StylePair[] = PAIRS): Promise<ParityResult> {
  const missing: string[] = [];
  const wanted = (side: "app" | "ref", list: StylePair[]): Wanted[] => list.map((p) => ({ key: p.name, target: p[side], props: propertiesOf(p.groups) }));

  // The app: a single context for the whole gallery.
  const appCtx = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor: 1, locale: "en-US", colorScheme: "dark" });
  let app: Record<string, Styles | null>;
  let galleryPng: Buffer;
  try {
    const page = await appCtx.newPage();
    await openGallery(page);
    await page.waitForSelector("[data-part]", { timeout: 10_000 });
    await settle(page);
    app = await read(page, wanted("app", pairs));
    galleryPng = await page.screenshot({ fullPage: true, animations: "disabled", caret: "hide" });
  } finally {
    await appCtx.close();
  }

  // The mockups: one per context (and per state), like `mockupRects` in `shots.ts`.
  const ref: Record<string, Styles | null> = {};
  const views = [...new Map(pairs.map((p) => [`${p.mockup}--${p.state ?? ""}`, { mockup: p.mockup, state: p.state }])).values()];
  for (const { mockup, state } of views) {
    const ctx = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor: 1, locale: "en-US", colorScheme: "dark" });
    try {
      await installRoutes(ctx);
      const page = await ctx.newPage();
      await page.goto(`${ORIGIN}/${mockup}.dc.html`);
      await waitForMockup(page);
      await settle(page);
      if (state) {
        const st = statesOf(mockup).find((s) => s.name === state);
        if (!st) throw new Error(`${mockup} has no state "${state}" (tools/mockup-render/src/states.ts)`);
        for (const step of st.steps) {
          if ("click" in step) await page.locator(step.click).click({ timeout: 5_000 });
          else await page.locator(step.fill).fill(step.text, { timeout: 5_000 });
          await settle(page);
        }
        // Without the pointer over the last button (its :hover would change the styles).
        await page.mouse.move(0, 0);
        await settle(page);
      }
      Object.assign(ref, await read(page, wanted("ref", pairs.filter((p) => p.mockup === mockup && p.state === state))));
    } finally {
      await ctx.close();
    }
  }

  const diffs: StyleDiff[] = [];
  for (const p of pairs) {
    const a = app[p.name];
    const m = ref[p.name];
    if (!a) missing.push(`${p.name}: is not in the gallery (${p.app.selector})`);
    if (!m) missing.push(`${p.name}: is not in ${p.mockup} (${p.ref.selector})`);
    if (a && m) diffs.push(...compareStyles(p.name, a, m, p.groups));
  }
  return { pairs, diffs, missing, galleryPng };
}
