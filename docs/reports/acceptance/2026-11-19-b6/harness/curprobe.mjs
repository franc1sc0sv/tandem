// What cursor events the UI of m1 receives (bridge SSE) while the engine crosses m1 -> m2.
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "r2");
async function entered(src, dst, [x, y, dx, dy], tries = 25) {
  for (let i = 0; i < tries; i++) {
    const before = (await dst.ctl({ cmd: "injected" })).count;
    await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90);
    const r = await dst.ctl({ cmd: "injected" });
    if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true;
  }
  return false;
}
const ev = []; const t0 = Date.now(); const ctl = new AbortController();
(async () => { const r = await fetch(`${m1.origin}/__bridge/events`, { signal: ctl.signal }); const rd = r.body.getReader(); const dec = new TextDecoder(); let buf = ""; for (;;) { const { value, done } = await rd.read(); if (done) break; buf += dec.decode(value); let i; while ((i = buf.indexOf("\n\n")) >= 0) { const chunk = buf.slice(0, i); buf = buf.slice(i + 2); try { const j = JSON.parse(chunk.replace(/^data: /, "")); if (/cursor|cross/.test(JSON.stringify(j).slice(0, 300))) ev.push([Date.now() - t0, j.event, JSON.stringify(j.payload).slice(0, 160)]); } catch {} } } })().catch(() => {});
const g = await L.geoLeftCenter(m1, m2);
if (process.argv[3] === "own") { for (let i = 0; i < 5; i++) { await m1.ctl({ cmd: "abs", x: 400 + i * 10, y: 400 }); await L.sleep(100); } await L.sleep(1500); }
console.log("server at watch time:", JSON.stringify((await m1.snapshot()).input?.server));
console.log("watch:", JSON.stringify(await m1.act({ kind: "watch_cursor", on: true })));
await L.sleep(1000);
console.log("server before:", JSON.stringify((await m1.snapshot()).input?.server), JSON.stringify((await m2.snapshot()).input?.server));
await m1.ctl({ cmd: "abs", x: 500, y: 500 }); await L.sleep(300);
const ok = await entered(m1, m2, g.m1m2); await L.sleep(800);
await m2.ctl({ cmd: "rel", dx: 30, dy: 0 }); await L.sleep(500);
console.log("crossed:", ok, "server after:", JSON.stringify((await m1.snapshot()).input?.server), JSON.stringify((await m2.snapshot()).input?.server));
console.log("events:", ev.length); for (const e of ev.slice(0, 6).concat(ev.slice(-6))) console.log(e.join(" "));
console.log("kinds:", JSON.stringify([...new Set(ev.map((e) => e[2].slice(0, 40)))]).slice(0, 600));
await m1.act({ kind: "watch_cursor", on: false }); ctl.abort();
