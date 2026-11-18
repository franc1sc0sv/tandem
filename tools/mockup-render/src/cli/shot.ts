// `pnpm mockups:shot [--out <folder>] [--only <mockup>]… [--no-states] [--concurrency N]`
// Renders the mockups in mockups/v03 and saves the screenshots (by default in docs/mockups-png).
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
    console.log(`FAIL  ${label}`);
    for (const i of issues) console.log(`      ${i}`);
  }
}

// Without `--only` and without failures, screenshots that match no mockup or state are deleted
// (against the full list, so `--no-states` does not delete the state ones). If something failed,
// nothing is touched: better one screenshot too many than losing a reference.
if (!values.only) {
  if (failed > 0) {
    console.log('\nthere were failures: no old screenshot is deleted');
  } else {
    for (const f of staleShots(readdirSync(outDir), jobsFor(listMockups()))) {
      rmSync(join(outDir, f));
      console.log(`deleted ${f} (that state no longer exists)`);
    }
  }
}

const secs = ((Date.now() - started) / 1000).toFixed(1);
console.log(`\n${results.length - failed}/${results.length} screenshots in ${relative(process.cwd(), outDir) || '.'} (${secs} s)`);
process.exit(failed > 0 ? 1 : 0);
