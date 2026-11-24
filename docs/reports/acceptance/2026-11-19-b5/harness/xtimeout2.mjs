// Same as xtimeout.mjs, but the cursor is left on the other machine first (a crossing just before).
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "r2"); const F = "F2-live-crossing-test";
const g = await L.geoLeftCenter(m1, m2);
for (let i = 0; i < 6; i++) { await m1.ctl({ cmd: "rel", dx: -2000, dy: 0 }); await L.sleep(60); }
await m1.ctl({ cmd: "abs", x: g.m1m2[0], y: g.m1m2[1] }); await m1.ctl({ cmd: "rel", dx: 8, dy: 0 }); await L.sleep(500);
const p = await L.openUi(m1, { hash: "#/arrangement/crossing" });
const t0 = Date.now(); const seen = [];
for (let i = 0; i < 40; i++) { const t = (await L.text(p)).replace(/\n+/g, " | "); const k = /Did not cross/.test(t) ? "timeout" : /Crossed correctly/.test(t) ? "crossed" : /Crossed at another/.test(t) ? "wrong" : /waiting for the cursor/.test(t) ? "waiting" : "other"; if (seen.at(-1)?.[1] !== k) seen.push([Math.round((Date.now() - t0) / 1000), k, t.slice(30, 200)]); await L.sleep(1000); }
console.log(JSON.stringify(seen, null, 1));
await L.shot(p, F, "xtimeout-cursor-on-other");
L.record(F, "crossing test with the cursor left on the other machine still ends (timeout or crossing) within 26 s", seen.some(([s, k]) => k !== "waiting" && k !== "other" && s <= 26), JSON.stringify(seen.map((x) => x.slice(0, 2))));
await L.closeBrowser();
