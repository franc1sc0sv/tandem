// Round 6, T-291: a just-paired peer must not be forgotten. Samples both agents every second for
// <secs> (past PAIRING_SETTLE, 60 s) and reads the agent logs. Usage: node t291.mjs <tag> <secs> <label>
import * as L from "./lib.mjs";
const tag = process.argv[2]; const secs = Number(process.argv[3] ?? 70); const label = process.argv[4] ?? "normal";
const { m1, m2 } = L.attach(tag);
const F = "T-291-no-spontaneous-unpair";
let samples = 0, bad = []; let linkless = 0;
for (let i = 0; i < secs; i++) {
  for (const m of [m1, m2]) {
    try { const s = await m.snapshot(); samples++; if (s.peer.kind !== "paired") bad.push(`${i}s ${m.label}=${s.peer.kind}`); else if (!s.peer.link) linkless++; }
    catch (e) { bad.push(`${i}s ${m.label} snapshot error ${String(e).slice(0, 80)}`); }
  }
  await L.sleep(1000);
}
const logs = [m1, m2].map((m) => m.agentLog());
const forgot = logs.map((l) => (l.match(/no longer trusts this one/g) ?? []).length);
const saving = logs.map((l) => (l.match(/still saving the pairing/g) ?? []).length);
const trust = [m1, m2].map((m) => /peer|key/.test(m.toml()) );
const ok = bad.length === 0 && forgot.every((n) => n === 0);
L.record(F, `${tag} (${label}): both stay paired for ${secs} s, nothing forgotten`, ok, JSON.stringify({ samples, bad: bad.slice(0, 10), linklessSamples: linkless, forgotLogs: forgot, stillSavingLogs: saving }));
