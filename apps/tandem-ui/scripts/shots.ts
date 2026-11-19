// App screenshot harness (T-112): builds the demo mode, captures each screen in
// headless Chromium the same way `tools/mockup-render` captures the mockups (no network, bundled
// fonts, finite animations finished and infinite ones in their initial state) and
// compares it with the mockup capture.
//
//   pnpm shots                    # todas → docs/screenshots/v03/<screen>.png + similarity.json
//   pnpm shots --only desk        # one
//   pnpm shots --check            # fails if a region with a threshold does not meet it
//   pnpm shots --no-build         # reutiliza dist-demo/
//   pnpm shots --diff-dir d/      # saves the differences of each comparison
//   pnpm shots --only styles      # only the component style parity (T-127)
//
// Besides the screens, it compares the computed styles of each gallery component
// (`gallery.html`) with the same element of the mockup (`style-parity-spec.ts`); with `--check`,
// a differing property also fails.
//
// Necesita `pnpm install` en tools/mockup-render (fuentes de los mockups) y Chromium de
// Playwright (`pnpm exec playwright install chromium`).

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { extname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { ORIGIN, installRoutes, statesOf, waitForMockup } from "@tandem/mockup-render";
import { capture, compare, compareWithMockup, launchBrowser, mockupShotPath, settle, type CompareResult } from "@tandem/mockup-render/harness";
import type { Browser, Page } from "playwright";
import { SHOTS, appUrl, crop, mask, type Rect, type ShotSpec } from "./shots-spec.ts";
import { runStyleParity } from "./style-parity.ts";
import { formatReport } from "./style-parity-spec.ts";

const APP_DIR = fileURLToPath(new URL("..", import.meta.url));
const REPO_DIR = join(APP_DIR, "..", "..");
const DIST = join(APP_DIR, "dist-demo");
const OUT = join(REPO_DIR, "docs", "screenshots", "v03");
const APP_ORIGIN = "http://app.local";

const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".svg": "image/svg+xml",
  ".png": "image/png",
};

interface Args {
  only: string[];
  check: boolean;
  build: boolean;
  diffDir: string | null;
}

function parseArgs(argv: string[]): Args {
  const a: Args = { only: [], check: false, build: true, diffDir: null };
  for (let i = 0; i < argv.length; i++) {
    const v = argv[i];
    if (v === "--only") a.only.push(...(argv[++i] ?? "").split(","));
    else if (v === "--check") a.check = true;
    else if (v === "--no-build") a.build = false;
    else if (v === "--diff-dir") a.diffDir = argv[++i] ?? null;
  }
  return a;
}

interface Measure {
  name: string;
  similarity: number;
  worstTile: number;
  min?: { similarity: number; worstTile: number };
  ok: boolean;
}

interface ShotResult {
  name: string;
  mockup: string;
  file: string;
  errors: string[];
  full: Measure;
  regions: Measure[];
}

/** Boxes of the selectors in the mockup (to mask and to crop regions). */
async function mockupRects(browser: Browser, spec: ShotSpec, selectors: string[]): Promise<Record<string, Rect | null>> {
  const context = await browser.newContext({ viewport: { width: spec.width, height: spec.height }, deviceScaleFactor: 1, locale: "en-US", colorScheme: "dark" });
  try {
    await installRoutes(context);
    const page = await context.newPage();
    await page.goto(`${ORIGIN}/${spec.mockup}.dc.html`);
    await waitForMockup(page);
    await settle(page);
    // Boxes are measured in the state of the capture (in "rollback" the text changes the card height).
    const state = spec.state ? statesOf(spec.mockup).find((x) => x.name === spec.state) : undefined;
    for (const step of state?.steps ?? []) {
      if ("click" in step) await page.locator(step.click).click({ timeout: 5_000 });
      else await page.locator(step.fill).fill(step.text, { timeout: 5_000 });
      await settle(page);
    }
    const out: Record<string, Rect | null> = {};
    for (const sel of selectors) {
      const box = await page.locator(sel).first().boundingBox({ timeout: 2_000 }).catch(() => null);
      out[sel] = box;
    }
    return out;
  } finally {
    await context.close();
  }
}

/** The boxes of all the elements of each selector. */
async function boxes(page: Page, selectors: string[]): Promise<Record<string, Rect[]>> {
  const out: Record<string, Rect[]> = {};
  for (const sel of selectors) {
    out[sel] = [];
    for (const el of await page.locator(sel).all()) {
      const b = await el.boundingBox();
      if (b) out[sel].push(b);
    }
  }
  return out;
}

/** Mockup boxes (all those of each selector). */
async function mockupBoxes(browser: Browser, spec: ShotSpec, selectors: string[]): Promise<Record<string, Rect[]>> {
  const context = await browser.newContext({ viewport: { width: spec.width, height: spec.height }, deviceScaleFactor: 1, locale: "en-US", colorScheme: "dark" });
  try {
    await installRoutes(context);
    const page = await context.newPage();
    await page.goto(`${ORIGIN}/${spec.mockup}.dc.html`);
    await waitForMockup(page);
    await settle(page);
    return await boxes(page, selectors);
  } finally {
    await context.close();
  }
}

/**
 * Compares only the content (`ShotSpec.content`): crops the app and the mockup from the corner
 * of their element, of the same size, and masks the allowed regions of each side in both.
 */
function compareContent(spec: ShotSpec, png: Buffer, appBoxes: Record<string, Rect[]>, mockBoxes: Record<string, Rect[]>, diffPath?: string): Measure {
  const c = spec.content;
  const a = appBoxes[c?.app ?? ""]?.[0];
  const m = mockBoxes[c?.mockup ?? ""]?.[0];
  if (!c || !a || !m) throw new Error(`${spec.name}: content not found in the app (${a ? "yes" : "no"}) or in the mockup (${m ? "yes" : "no"})`);
  const appImg = pngSize(png);
  const width = Math.floor(Math.min(a.width, m.width));
  const height = Math.floor(Math.min(appImg.height - a.y, spec.height - m.y));
  const rel = (r: Rect, o: Rect): Rect => ({ x: r.x - o.x, y: r.y - o.y, width: r.width, height: r.height });
  const holes: Rect[] = [];
  for (const k of c.masks) {
    for (const r of appBoxes[k.app] ?? []) holes.push(rel(r, a));
    if (k.mockup) for (const r of mockBoxes[k.mockup] ?? []) holes.push(rel(r, m));
  }
  const app = mask(crop(png, { x: a.x, y: a.y, width, height }), holes);
  const ref = mask(crop(readFileSync(mockupShotPath(spec.mockup, spec.state)), { x: m.x, y: m.y, width, height }), holes);
  const r = compare(app, ref);
  if (diffPath) writeFileSync(diffPath, r.diffPng);
  const ok = r.similarity >= c.min.similarity && r.worstTile.similarity >= c.min.worstTile;
  return { name: "content", similarity: round(r.similarity), worstTile: round(r.worstTile.similarity), min: c.min, ok };
}

function pngSize(png: Buffer): { width: number; height: number } {
  return { width: png.readUInt32BE(16), height: png.readUInt32BE(20) };
}

async function serveApp(page: Page, errors: string[]): Promise<void> {
  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== APP_ORIGIN) {
      errors.push(`external request blocked: ${url.href}`);
      return route.abort("blockedbyclient");
    }
    const rel = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, "") || "index.html";
    const path = join(DIST, rel);
    if (!path.startsWith(DIST + sep) || !existsSync(path)) return route.fulfill({ status: 404, body: "not found" });
    return route.fulfill({ body: readFileSync(path), contentType: TYPES[extname(path)] ?? "application/octet-stream" });
  });
}

async function shoot(browser: Browser, spec: ShotSpec, args: Args): Promise<ShotResult> {
  const errors: string[] = [];
  const context = await browser.newContext({
    viewport: { width: spec.content?.appWidth ?? spec.width, height: spec.height },
    deviceScaleFactor: 1,
    locale: "en-US",
    timezoneId: "UTC",
    colorScheme: "dark",
    reducedMotion: "no-preference",
    // The mockups are drawn on macOS (⌘K): the app is captured with that shortcut.
    userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
  });
  let png: Buffer;
  let appBoxes: Record<string, Rect[]> = {};
  try {
    const page = await context.newPage();
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(`console: ${m.text()}`);
    });
    page.on("pageerror", (e) => errors.push(`exception: ${e.message}`));
    await serveApp(page, errors);
    await page.goto(appUrl(APP_ORIGIN, spec));
    await page.waitForSelector("#app > *", { timeout: 10_000 });
    if (spec.content?.appStyle) await page.addStyleTag({ content: spec.content.appStyle });
    await settle(page);
    for (const f of spec.fills ?? []) {
      await page.locator(f.selector).first().fill(f.text, { timeout: 5_000 });
      await settle(page);
    }
    // The same clicks that lead to the mockup state (`tools/mockup-render/src/states.ts`).
    for (const sel of spec.clicks ?? []) {
      await page.locator(sel).click({ timeout: 5_000 });
      await settle(page);
    }
    if ((spec.clicks?.length ?? 0) > 0) {
      // Same as the mockup renderer: without the pointer over the last button (its :hover would change the capture).
      await page.mouse.move(0, 0);
      await settle(page);
    }
    for (const k of spec.keys ?? []) {
      await page.keyboard.press(k);
      await settle(page);
    }
    if (spec.unhover) {
      await page.mouse.move(0, 0);
      await settle(page);
    }
    const text = await page.evaluate(() => document.body.innerText);
    for (const bad of ["{{", "undefined", "NaN"]) if (text.includes(bad)) errors.push(`visible text with “${bad}”`);
    png = await capture(page);
    if (spec.content) appBoxes = await boxes(page, [spec.content.app, ...spec.content.masks.map((k) => k.app)]);
  } finally {
    await context.close();
  }

  const file = join(OUT, `${spec.name}.png`);
  writeFileSync(file, png);
  if (spec.content) {
    const sels = [spec.content.mockup, ...spec.content.masks.flatMap((k) => (k.mockup ? [k.mockup] : []))];
    const mock = await mockupBoxes(browser, spec, sels);
    const diffPath = args.diffDir ? join(args.diffDir, `${spec.name}.diff.png`) : undefined;
    const content = compareContent(spec, png, appBoxes, mock, diffPath);
    return { name: spec.name, mockup: spec.state ? `${spec.mockup}--${spec.state}` : spec.mockup, file, errors, full: content, regions: [content] };
  }
  // With masked regions, comparison is against the equally masked reference; otherwise directly against the
  // mockup capture (`compareWithMockup`).
  let ref: Buffer | null = null;

  const ignoreSel = (spec.ignore ?? []).flatMap((i) => (i.mockupSelector ? [i.mockupSelector] : []));
  const regionSel = (spec.regions ?? []).flatMap((r) => (r.mockupSelector ? [r.mockupSelector] : []));
  const rects = ignoreSel.length + regionSel.length > 0 ? await mockupRects(browser, spec, [...ignoreSel, ...regionSel]) : {};
  const masked = [...ignoreSel.map((s) => rects[s]), ...(spec.ignore ?? []).map((i) => i.rect)].filter((r): r is Rect => r !== null && r !== undefined);
  let app = png;
  if (masked.length > 0) {
    app = mask(app, masked);
    ref = mask(readFileSync(mockupShotPath(spec.mockup, spec.state)), masked);
  }

  const measure = (name: string, region: Rect | undefined, min?: Measure["min"]): Measure => {
    const diffPath = args.diffDir ? join(args.diffDir, `${spec.name}${name === "screen" ? "" : `--${name.replace(/\s+/g, "-")}`}.diff.png`) : undefined;
    const box = region ? { x: Math.floor(region.x), y: Math.floor(region.y), width: Math.floor(region.width), height: Math.floor(region.height) } : undefined;
    let r: CompareResult;
    if (ref) {
      r = compare(app, ref, { region: box });
      if (diffPath) writeFileSync(diffPath, r.diffPng);
    } else {
      r = compareWithMockup(app, { mockup: spec.mockup, state: spec.state, region: box, diffPath });
    }
    const ok = !min || (r.similarity >= min.similarity && r.worstTile.similarity >= min.worstTile);
    return { name, similarity: round(r.similarity), worstTile: round(r.worstTile.similarity), min, ok };
  };

  const full = measure("screen", undefined, spec.min);
  const regions: Measure[] = [];
  for (const r of spec.regions ?? []) {
    const rect = r.rect ?? (r.mockupSelector ? rects[r.mockupSelector] : null);
    if (!rect) {
      errors.push(`region “${r.name}”: not found in the mockup`);
      continue;
    }
    regions.push(measure(r.name, rect, r.min));
  }
  return { name: spec.name, mockup: spec.state ? `${spec.mockup}--${spec.state}` : spec.mockup, file, errors, full, regions };
}

const round = (n: number) => Math.round(n * 10000) / 10000;

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  if (args.build) execFileSync("pnpm", ["exec", "vite", "build", "--mode", "demo", "--outDir", "dist-demo", "--logLevel", "warn"], { cwd: APP_DIR, stdio: "inherit" });
  mkdirSync(OUT, { recursive: true });
  if (args.diffDir) mkdirSync(args.diffDir, { recursive: true });
  const specs = args.only.length ? SHOTS.filter((s) => args.only.includes(s.name)) : SHOTS;
  const styles = args.only.length === 0 || args.only.includes("styles");
  // Same Chromium options as the mockup captures (`--disable-partial-raster`).
  const browser = await launchBrowser();
  const results: ShotResult[] = [];
  let styleFailed = false;
  try {
    for (const s of specs) results.push(await shoot(browser, s, args));
    if (styles) {
      const errors: string[] = [];
      const parity = await runStyleParity(browser, async (page) => {
        page.on("pageerror", (e) => errors.push(`exception: ${e.message}`));
        await serveApp(page, errors);
        await page.goto(`${APP_ORIGIN}/gallery.html`);
      });
      writeFileSync(join(OUT, "gallery.png"), parity.galleryPng);
      console.log(`\nComponent styles (gallery ↔ mockups):\n${formatReport(parity.pairs, parity.diffs, parity.missing)}`);
      for (const e of errors) console.log(`! gallery: ${e}`);
      styleFailed = parity.diffs.length > 0 || parity.missing.length > 0 || errors.length > 0;
    }
  } finally {
    await browser.close();
  }

  const fmt = (m: Measure) => `${(m.similarity * 100).toFixed(2)} % (worst tile ${(m.worstTile * 100).toFixed(1)} %)${m.min ? (m.ok ? " ✓" : " ✗") : ""}`;
  for (const r of results) {
    console.log(`${r.name.padEnd(10)} ${r.mockup.padEnd(13)} ${fmt(r.full)}`);
    for (const g of r.regions) console.log(`${"".padEnd(10)} ${("· " + g.name).padEnd(13)} ${fmt(g)}`);
    for (const e of r.errors) console.log(`${"".padEnd(10)} ! ${e}`);
  }

  // The summary is merged with the existing one, so `--only` does not erase the other screens.
  const summaryFile = join(OUT, "similarity.json");
  const prev: Record<string, unknown> = existsSync(summaryFile) ? (JSON.parse(readFileSync(summaryFile, "utf8")) as Record<string, unknown>) : {};
  for (const r of results) prev[r.name] = { mockup: r.mockup, screen: r.full, regions: r.regions, errors: r.errors };
  const sorted = Object.fromEntries(Object.entries(prev).sort(([a], [b]) => a.localeCompare(b)));
  writeFileSync(summaryFile, JSON.stringify(sorted, null, 2) + "\n");

  const failed = results.filter((r) => r.errors.length > 0 || !r.full.ok || r.regions.some((g) => !g.ok));
  if (args.check && (failed.length > 0 || styleFailed)) {
    console.error(`\nFailing: ${[...failed.map((r) => r.name), ...(styleFailed ? ["styles"] : [])].join(", ")}`);
    process.exit(1);
  }
}

await main();
