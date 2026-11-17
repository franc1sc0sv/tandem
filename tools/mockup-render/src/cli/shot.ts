// `pnpm mockups:shot [--out <carpeta>] [--only <mockup>]… [--no-states] [--concurrency N]`
// Renderiza los mockups de mockups/v03 y guarda las capturas (por omisión en docs/mockups-png).
import { readdirSync, rmSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { listMockups, SHOTS_DIR, shotName } from '../paths.ts';
import { jobsFor, parseConcurrency, problems, renderMockups, staleShots } from '../render.ts';

const { values } = parseArgs({
  options: {
    out: { type: 'string' },
    only: { type: 'string', multiple: true },
    'no-states': { type: 'boolean', default: false },
    concurrency: { type: 'string', default: '4' },
  },
});

let concurrency: number;
try {
  concurrency = parseConcurrency(values.concurrency);
} catch (e) {
  console.error(`mockups:shot: ${(e as Error).message}`);
  process.exit(2);
}

const outDir = resolve(values.out ?? SHOTS_DIR);
const withStates = !values['no-states'];
const started = Date.now();
const results = await renderMockups({ outDir, names: values.only, states: withStates, concurrency });

let failed = 0;
for (const r of results) {
  const label = shotName(r.mockup, r.state);
  const issues = problems(r);
  if (issues.length === 0) {
    console.log(`ok    ${label}`);
  } else {
    failed += 1;
    console.log(`FALLA ${label}`);
    for (const i of issues) console.log(`      ${i}`);
  }
}

// Sin `--only` y sin fallos, se borran las capturas que no corresponden a ningún mockup o estado
// (contra la lista completa, así `--no-states` no borra las de los estados). Si algo falló, no
// se toca nada: mejor una captura de más que perder una referencia.
if (!values.only) {
  if (failed > 0) {
    console.log('\nhubo fallos: no se borra ninguna captura vieja');
  } else {
    for (const f of staleShots(readdirSync(outDir), jobsFor(listMockups()))) {
      rmSync(join(outDir, f));
      console.log(`borrada ${f} (ya no hay ese estado)`);
    }
  }
}

const secs = ((Date.now() - started) / 1000).toFixed(1);
console.log(`\n${results.length - failed}/${results.length} capturas en ${relative(process.cwd(), outDir) || '.'} (${secs} s)`);
process.exit(failed > 0 ? 1 : 0);
