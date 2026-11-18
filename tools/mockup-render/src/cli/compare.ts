// `pnpm mockups:compare <app.png> <mockup.png> [--diff diff.png] [--min 0.99]`
//   Similarity between two screenshots; exits with 1 if it falls below --min.
// `pnpm mockups:compare --dirs <expected> <actual> [--min 0.99] [--diff-dir <folder>]`
//   Compares same-named screenshots from two folders and writes a Markdown table.
import { mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { compare } from '../compare.ts';

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    diff: { type: 'string' },
    'diff-dir': { type: 'string' },
    dirs: { type: 'boolean', default: false },
    min: { type: 'string', default: '0.99' },
  },
});

const min = Number(values.min);
if (values.min.trim() === '' || !(min >= 0 && min <= 1)) {
  console.error(`mockups:compare: --min must be a number between 0 and 1 (got ${JSON.stringify(values.min)})`);
  process.exit(2);
}
const [first, second] = positionals;
if (!first || !second) {
  console.error('uso: mockups:compare <app.png> <mockup.png> [--diff diff.png] [--min 0.99]');
  console.error('     mockups:compare --dirs <expected> <actual> [--min 0.99] [--diff-dir <folder>]');
  process.exit(2);
}

const pct = (x: number) => (x * 100).toFixed(2) + ' %';

if (!values.dirs) {
  const r = compare(first, second);
  if (values.diff) writeFileSync(values.diff, r.diffPng);
  const t = r.worstTile;
  console.log(`similarity ${pct(r.similarity)} (${r.mismatched} of ${r.total} pixels different)`);
  console.log(`worst tile ${pct(t.similarity)} en x=${t.x} y=${t.y} (${t.width}×${t.height})`);
  process.exit(r.similarity >= min ? 0 : 1);
}

const expected = new Set(readdirSync(first).filter((f) => f.endsWith('.png')));
const actual = new Set(readdirSync(second).filter((f) => f.endsWith('.png')));
const rows: string[] = [];
let below = 0;
if (values['diff-dir']) mkdirSync(values['diff-dir'], { recursive: true });
for (const name of [...new Set([...expected, ...actual])].sort()) {
  if (!expected.has(name) || !actual.has(name)) {
    below += 1;
    rows.push(`| ${name} | ${expected.has(name) ? 'actual missing' : 'expected missing'} | |`);
    continue;
  }
  try {
    const r = compare(join(second, name), join(first, name));
    if (r.similarity < min) {
      below += 1;
      if (values['diff-dir']) writeFileSync(join(values['diff-dir'], name), r.diffPng);
    }
    const sim = r.similarity < min ? '**' + pct(r.similarity) + '**' : pct(r.similarity);
    rows.push(`| ${name} | ${sim} | ${pct(r.worstTile.similarity)} |`);
  } catch (e) {
    below += 1;
    rows.push(`| ${name} | ${(e as Error).message} | |`);
  }
}
console.log(`| Screenshot | Similarity | Worst tile |\n|---|---|---|\n${rows.join('\n')}\n`);
console.log(below === 0 ? `All ≥ ${pct(min)}.` : `${below} below ${pct(min)} or unpaired.`);
process.exit(below === 0 ? 0 : 1);
