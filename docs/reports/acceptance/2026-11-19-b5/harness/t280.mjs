// T-280 repro: a monitor first seen only by side B (B owns it and reads it), then also by side A
// (A wins the election). Afterwards B must show A's report; the bug leaves it null until the input changes.
// Usage: node t280.mjs <tag> [runs]
import * as L from "./lib.mjs";
const tag = process.argv[2]; const R = Number(process.argv[3] ?? 5); const { m1, m2 } = L.attach(tag);
const F = "T-280-report-lost-on-owner-change";
const [A, B] = (await m1.snapshot()).agent === "a" ? [m1, m2] : [m2, m1];
let lost = 0; const out = [];
for (let i = 0; i < R; i++) {
  const serial = 900 + i + Math.floor(Math.random() * 1000) * 10; const name = `Rpt-${serial}`;
  // Mode "seq": B first, A 7 s later. Mode "same" (default): A then B at once, like t3.mjs.
  if (process.argv[4] === "seq") { await B.ctl({ cmd: "monitor_add", personality: "standard", name, serial, input: "dp", via: "hdmi" }); await L.sleep(7000); await A.ctl({ cmd: "monitor_add", personality: "standard", name, serial, input: "dp", via: "dp" }); }
  else { await A.ctl({ cmd: "monitor_add", personality: "standard", name, serial, input: "dp", via: "dp" }); await L.sleep(Math.floor(Math.random() * 5000)); await B.ctl({ cmd: "monitor_add", personality: "standard", name, serial, input: "dp", via: "hdmi" }); }
  await L.sleep(12000);
  const vb = (await B.snapshot()).monitors.find((m) => m.name === name), va = (await A.snapshot()).monitors.find((m) => m.name === name);
  const r = { name, ownerOnA: va?.owner, ownerOnB: vb?.owner, reportOnA: va?.report?.input ?? null, reportOnB: vb?.report?.input ?? null };
  if (r.ownerOnB === "a" && r.reportOnA && !r.reportOnB) lost++;
  out.push(r); console.log(JSON.stringify(r));
}
L.record(F, `after the owner moves from B to A, B keeps A's report of the monitor (${R - lost}/${R})`, lost === 0, JSON.stringify(out).slice(0, 700));
