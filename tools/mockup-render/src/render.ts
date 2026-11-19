import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import type { Browser } from 'playwright';
import { ORIGIN, capture, checkPage, installRoutes, launchBrowser, settle, waitForMockup } from './browser.ts';
import { MOCKUP_SUFFIX, MOCKUPS_DIR, listMockups, mockupViewport, shotName } from './paths.ts';
import { type Step, statesOf } from './states.ts';

export interface RenderJob {
  mockup: string;
  state?: string;
  steps: Step[];
}

export interface RenderResult {
  mockup: string;
  state?: string;
  /** Screenshot path, if `outDir` was requested. */
  file?: string;
  consoleErrors: string[];
  pageErrors: string[];
  blocked: string[];
  stepErrors: string[];
  braces: string[];
  dcErrors: string[];
  dcMissing: string[];
  fontErrors: string[];
}

export interface RenderOptions {
  /** Mockups to render (by default, all those in `dir`). */
  names?: string[];
  dir?: string;
  /** Where to save the screenshots; without it they are only checked. */
  outDir?: string;
  /** Include the interactive states of `states.ts` (default yes). */
  states?: boolean;
  /** Pages in parallel (integer ≥ 1; default 4). */
  concurrency?: number;
  /** Already open browser (the tests reuse it); open it with `launchBrowser()`. */
  browser?: Browser;
}

/** Problems of a result; empty = the mockup rendered fine. */
export function problems(r: RenderResult): string[] {
  return [
    ...r.consoleErrors.map((e) => `console: ${e}`),
    ...r.pageErrors.map((e) => `exception: ${e}`),
    ...r.blocked.map((e) => `blocked request: ${e}`),
    ...r.stepErrors.map((e) => `paso: ${e}`),
    ...r.braces.map((e) => `unsubstituted: ${e}`),
    ...r.dcErrors.map((e) => `runtime: ${e}`),
    ...r.dcMissing.map((e) => `missing value: ${e}`),
    ...r.fontErrors.map((e) => `font: ${e}`),
  ];
}

/** Validates `--concurrency` / `concurrency`: an integer ≥ 1, or an error that says what arrived. */
export function parseConcurrency(value: string | number): number {
  const n = typeof value === 'number' ? value : value.trim() === '' ? NaN : Number(value);
  if (!Number.isInteger(n) || n < 1) {
    throw new RangeError(`concurrency must be an integer ≥ 1 (got ${JSON.stringify(value)})`);
  }
  return n;
}

/**
 * Screenshots in `existing` (file names) that match no job in `jobs`:
 * those of a state or mockup that no longer exists. It comes from the jobs, not the results,
 * so a job that failed before capturing does not get its reference deleted.
 */
export function staleShots(existing: string[], jobs: RenderJob[]): string[] {
  const keep = new Set(jobs.map((j) => shotName(j.mockup, j.state)));
  return existing.filter((f) => f.endsWith('.png') && !keep.has(f)).sort();
}

export function jobsFor(names: string[], withStates = true): RenderJob[] {
  return names.flatMap((mockup) => [
    { mockup, steps: [] },
    ...(withStates ? statesOf(mockup).map((s) => ({ mockup, state: s.name, steps: s.steps })) : []),
  ]);
}

async function runJob(browser: Browser, job: RenderJob, dir: string, outDir?: string): Promise<RenderResult> {
  const result: RenderResult = {
    mockup: job.mockup,
    state: job.state,
    consoleErrors: [],
    pageErrors: [],
    blocked: [],
    stepErrors: [],
    braces: [],
    dcErrors: [],
    dcMissing: [],
    fontErrors: [],
  };
  const context = await browser.newContext({
    viewport: mockupViewport(job.mockup, dir),
    deviceScaleFactor: 1,
    locale: 'en-US',
    timezoneId: 'UTC',
    colorScheme: 'dark',
    reducedMotion: 'no-preference',
  });
  try {
    await installRoutes(context, { dir, onBlocked: (url) => result.blocked.push(url) });
    const page = await context.newPage();
    page.on('console', (m) => {
      if (m.type() === 'error') result.consoleErrors.push(m.text());
    });
    page.on('pageerror', (e) => result.pageErrors.push(e.message));
    await page.goto(`${ORIGIN}/${job.mockup}${MOCKUP_SUFFIX}`);
    await waitForMockup(page);
    await settle(page);
    for (const step of job.steps) {
      try {
        if ('click' in step) await page.locator(step.click).click({ timeout: 5_000 });
        else await page.locator(step.fill).fill(step.text, { timeout: 5_000 });
      } catch (e) {
        result.stepErrors.push(`${JSON.stringify(step)}: ${(e as Error).message.split('\n')[0]}`);
        break;
      }
      await settle(page);
    }
    if (job.steps.length > 0) {
      // Without the pointer over the last button (its :hover would change the screenshot).
      await page.mouse.move(0, 0);
      await settle(page);
    }
    const check = await checkPage(page);
    result.braces = check.braces;
    result.dcErrors = check.dcErrors;
    result.dcMissing = check.dcMissing;
    result.fontErrors = check.fontErrors;
    if (outDir) {
      result.file = join(outDir, shotName(job.mockup, job.state));
      await capture(page, result.file);
    }
  } catch (e) {
    result.pageErrors.push((e as Error).message.split('\n')[0] ?? String(e));
  } finally {
    await context.close();
  }
  return result;
}

/** Renders mockups (and their states) in headless Chromium; optionally saves the screenshots. */
export async function renderMockups(opts: RenderOptions = {}): Promise<RenderResult[]> {
  const concurrency = parseConcurrency(opts.concurrency ?? 4);
  const dir = opts.dir ?? MOCKUPS_DIR;
  const jobs = jobsFor(opts.names ?? listMockups(dir), opts.states ?? true);
  if (opts.outDir) mkdirSync(opts.outDir, { recursive: true });
  const browser = opts.browser ?? (await launchBrowser());
  const results: RenderResult[] = new Array(jobs.length);
  let next = 0;
  const worker = async () => {
    while (next < jobs.length) {
      const i = next++;
      const job = jobs[i];
      if (job) results[i] = await runJob(browser, job, dir, opts.outDir);
    }
  };
  try {
    await Promise.all(Array.from({ length: concurrency }, worker));
  } finally {
    if (!opts.browser) await browser.close();
  }
  return results;
}
