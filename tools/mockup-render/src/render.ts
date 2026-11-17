import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { chromium, type Browser } from 'playwright';
import { ORIGIN, checkPage, installRoutes, settle, waitForMockup } from './browser.ts';
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
  /** Ruta de la captura, si se pidió `outDir`. */
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
  /** Mockups a renderizar (por omisión, todos los de `dir`). */
  names?: string[];
  dir?: string;
  /** Dónde guardar las capturas; sin esto solo se comprueba. */
  outDir?: string;
  /** Incluir los estados interactivos de `states.ts` (por omisión, sí). */
  states?: boolean;
  /** Páginas en paralelo. */
  concurrency?: number;
  /** Navegador ya abierto (las pruebas lo reutilizan). */
  browser?: Browser;
}

/** Problemas de un resultado; vacío = el mockup renderizó bien. */
export function problems(r: RenderResult): string[] {
  return [
    ...r.consoleErrors.map((e) => `consola: ${e}`),
    ...r.pageErrors.map((e) => `excepción: ${e}`),
    ...r.blocked.map((e) => `petición bloqueada: ${e}`),
    ...r.stepErrors.map((e) => `paso: ${e}`),
    ...r.braces.map((e) => `sin sustituir: ${e}`),
    ...r.dcErrors.map((e) => `runtime: ${e}`),
    ...r.dcMissing.map((e) => `valor ausente: ${e}`),
    ...r.fontErrors.map((e) => `fuente: ${e}`),
  ];
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
    locale: 'es-ES',
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
      // Sin el puntero encima del último botón (su :hover cambiaría la captura).
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
      await page.screenshot({ path: result.file, animations: 'disabled', caret: 'hide' });
    }
  } catch (e) {
    result.pageErrors.push((e as Error).message.split('\n')[0] ?? String(e));
  } finally {
    await context.close();
  }
  return result;
}

/** Renderiza mockups (y sus estados) en Chromium headless; opcionalmente guarda las capturas. */
export async function renderMockups(opts: RenderOptions = {}): Promise<RenderResult[]> {
  const dir = opts.dir ?? MOCKUPS_DIR;
  const jobs = jobsFor(opts.names ?? listMockups(dir), opts.states ?? true);
  if (opts.outDir) mkdirSync(opts.outDir, { recursive: true });
  const browser = opts.browser ?? (await chromium.launch());
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
    await Promise.all(Array.from({ length: Math.max(1, opts.concurrency ?? 4) }, worker));
  } finally {
    if (!opts.browser) await browser.close();
  }
  return results;
}
