// Prints each monitor's owner and report as both agents see them. Usage: node mon.mjs <tag>
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2]);
for (const m of [m1, m2]) { const s = await m.snapshot(); console.log(m.label, s.agent, JSON.stringify(s.monitors.map((x) => [x.id, x.owner, x.report]))); }
