// The Arrangement crossing test with no crossing: when does "Did not cross in 20 s" show?
import * as L from "./lib.mjs";
const { m1 } = L.attach(process.argv[2] ?? "r2"); const F = "F2-live-crossing-test";
const p = await L.openUi(m1, { hash: "#/arrangement/crossing" });
const t0 = Date.now(); const seen = [];
for (let i = 0; i < 36; i++) { const t = (await L.text(p)).replace(/\n+/g, " | "); const k = /Did not cross/.test(t) ? "timeout" : /Crossed correctly/.test(t) ? "crossed" : /Crossed at another/.test(t) ? "wrong" : /waiting for the cursor/.test(t) ? "waiting" : "other"; if (seen.at(-1)?.[1] !== k) seen.push([Math.round((Date.now() - t0) / 1000), k]); await L.sleep(1000); }
console.log(JSON.stringify(seen));
await L.shot(p, F, process.argv[3] ?? "xtimeout");
L.record(F, "crossing test with no crossing ends with \"Did not cross in 20 s\" (fresh window)", seen.some(([s, k]) => k === "timeout" && s <= 26), JSON.stringify(seen));
await L.closeBrowser();
