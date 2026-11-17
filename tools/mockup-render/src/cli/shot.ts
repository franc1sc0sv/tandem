// `pnpm mockups:shot [--out <carpeta>] [--only <mockup>]… [--no-states] [--concurrency N]`
// Renderiza los mockups de mockups/v03 y guarda las capturas (por omisión en docs/mockups-png).
import { readdirSync, rmSync } from 'node:fs';
import { basename, join, relative, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { SHOTS_DIR, shotName } from '../paths.ts';
import { problems, renderMockups } from '../render.ts';

const { values } = parseArgs({
  options: {
    out: { type: 'string' },
    only: { type: 'string', multiple: true },
    'no-states': { type: 'boolean', default: false },
    concurrency: { type: 'string', default: '4' },
  },
});

const outDir = resolve(values.out ?? SHOTS_DIR);
const started = Date.now();
const results = await renderMockups({
  outDir,
  names: values.only,
  states: !values['no-states'],
  concurrency: Number(values.concurrency),
});

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

// Con la corrida completa, se borran las capturas que ya no corresponden a ningún estado.
if (!values.only) {
  const keep = new Set(results.map((r) => basename(r.file ?? '')));
  for (const f of readdirSync(outDir)) {
    if (f.endsWith('.png') && !keep.has(f)) {
      rmSync(join(outDir, f));
      console.log(`borrada ${f} (ya no hay ese estado)`);
    }
  }
}

const secs = ((Date.now() - started) / 1000).toFixed(1);
console.log(`\n${results.length - failed}/${results.length} capturas en ${relative(process.cwd(), outDir) || '.'} (${secs} s)`);
process.exit(failed > 0 ? 1 : 0);
