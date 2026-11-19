import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host"); const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Steps] button", { hasText: s }).click();
async function entered(src, dst, [x, y, dx, dy], tries = 25) {
  for (let i = 0; i < tries; i++) {
    const before = (await dst.ctl({ cmd: "injected" })).count;
    await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90);
    const r = await dst.ctl({ cmd: "injected" });
    if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true;
  }
  return false;
}
const ev = []; const t0 = Date.now(); const ac = new AbortController();
(async () => { const r = await fetch(`${m1.origin}/__bridge/events`, { signal: ac.signal }); const rd = r.body.getReader(); const dec = new TextDecoder(); let buf = ""; for (;;) { const { value, done } = await rd.read(); if (done) break; buf += dec.decode(value); let i; while ((i = buf.indexOf("\n\n")) >= 0) { const chunk = buf.slice(0, i); buf = buf.slice(i + 2); try { const j = JSON.parse(chunk.replace(/^data: /, "")); const sj = JSON.stringify(j.payload ?? "").slice(0, 160); if (/cursor|cross/.test(sj)) ev.push([Date.now() - t0, sj]); } catch {} } } })().catch(() => {});
try {
  const g = await L.geoLeftCenter(m1, m2);
  // bring the cursor home: if an earlier run left it on the other machine, motion on m1 is only forwarded there
  // (and would count as a crossing in entered()); moving far left crosses back to m1's screen.
  for (let i = 0; i < 6; i++) { await m1.ctl({ cmd: "rel", dx: g.m1m2[2] > 0 ? -2000 : 2000, dy: 0 }); await L.sleep(60); } // round 6: away from the edge toward the other machine (m1 may be either side)
  await m1.ctl({ cmd: "abs", x: 600, y: 500 }); await L.sleep(800);
  const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1000);
  await p.getByRole("button", { name: /Open/ }).first().click();
  await L.until("w", async () => (await wiz(p).count()) === 1);
  await step(p, "Arrangement and crossing test"); await L.sleep(1200);
  const t5 = (await wiz(p).locator("main").innerText()).replace(/\n+/g, " | ");
  console.log("PASO5:", t5.slice(0, 900));
  L.record(F, "step 5: the arrangement shows the real resolution, aspect ratio and scale of the screens", /1920/.test(t5) && /3440/.test(t5) && !/—/.test(t5.replace(/Size[^|]*\|[^|]*—/, "")), t5.slice(0, 300));
  await L.shot(p, F, "10-step5");
  await wiz(p).getByRole("button", { name: /Test crossing/ }).click(); await L.sleep(1200);
  const ok = await entered(m1, m2, g.m1m2); await L.sleep(1800);
  const t5b = (await wiz(p).locator("main").innerText()).replace(/\n+/g, " | ");
  console.log("cursor events:", ev.length, ev.slice(-8).map((e) => e.join(" ")).join("\n")); console.log("server:", JSON.stringify((await m1.snapshot()).input?.server), "calls:", JSON.stringify((await m1.calls())).slice(-300));
  const cont = wiz(p).getByRole("button", { name: /Continue/ });
  const enabled = await cont.isEnabled();
  L.record(F, "step 5: a real crossing passes the test and enables \"Continue\"", ok && /Crossed to .* through the (right|left) edge/.test(t5b) && enabled, `crossing=${ok} continue=${enabled}; ${t5b.slice(0, 300)}`);
  await L.shot(p, F, "11-step5-crossed");
  await step(p, "Done"); await L.sleep(800);
  const t6 = (await wiz(p).locator("main").innerText()).replace(/\n+/g, " | ");
  L.record(F, "summary: says \"crossing tested\" after the real crossing", /crossing tested/i.test(t6) && !/crossing not tested/.test(t6), t6.slice(0, 400));
  await L.shot(p, F, "12-summary");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
ac.abort(); setTimeout(() => process.exit(0), 200);
