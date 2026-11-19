// Round 6, T-280: right after setup, both agents must hold the owner's report of every shared monitor
// (Std-34 and Zfail-27, plugged into both). Usage: node t280r.mjs <tag>
import * as L from "./lib.mjs";
const tag = process.argv[2]; const { m1, m2 } = L.attach(tag);
const F = "T-280-report-after-pairing";
const rows = [];
for (const m of [m1, m2]) {
  const s = await m.snapshot();
  for (const v of s.monitors.filter((x) => x.name !== "Factory-32")) rows.push({ m: m.label, side: s.agent, id: v.id, owner: v.owner, report: v.report ? v.report.input : null, peer: s.peer.kind });
}
const ok = rows.length === 4 && rows.every((r) => r.owner && r.report && r.peer === "paired");
L.record(F, `${tag}: both agents have the owner's report of every shared monitor`, ok, JSON.stringify(rows));
