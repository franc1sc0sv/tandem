import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F2-live-crossing-test";
async function entered(src, dst, [x, y, dx, dy], tries = 25) {
  for (let i = 0; i < tries; i++) {
    const before = (await dst.ctl({ cmd: "injected" })).count;
    await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90);
    const r = await dst.ctl({ cmd: "injected" });
    if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true;
  }
  return false;
}
try {
  const g = await L.geoLeftCenter(m1, m2);
  // bring the cursor home: if an earlier run left it on the other machine, motion on m1 is only forwarded there
  // (and would count as a crossing in entered()); moving far left crosses back to m1's screen.
  for (let i = 0; i < 6; i++) { await m1.ctl({ cmd: "rel", dx: -2000, dy: 0 }); await L.sleep(60); }
  await m1.ctl({ cmd: "abs", x: 600, y: 500 }); await L.sleep(800);
  // 1) good crossing: through the arrangement edge
  const p = await L.openUi(m1, { hash: "#/arrangement" }); await L.sleep(1200);
  const btn = p.locator("a:has-text('Test crossing'), button:has-text('Test crossing')").first();
  const disabled = await btn.evaluate((e) => e.disabled === true || e.getAttribute("aria-disabled") === "true");
  L.record(F, "Arrangement: \"Test crossing\" is enabled with the real agent", !disabled);
  await btn.click(); await L.sleep(1200);
  const t0 = (await L.text(p)).replace(/\n+/g, " | ");
  console.log("waiting:", t0.slice(0, 400));
  await L.shot(p, F, "10-waiting");
  const ok = await entered(m1, m2, g.m1m2);
  await L.sleep(1800);
  const t1 = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "crossing test: a real engine crossing through the arrangement edge gives \"Crossed OK\"", ok && /Crossed correctly/.test(t1), `crossing=${ok}; ${t1.slice(0, 400)}`);
  await L.shot(p, F, "11-crossed-ok");
  // 2) no crossing: 20 s
  await p.goto(m1.origin + "/#/arrangement/crossing"); await p.reload(); const tr = Date.now(); let t2x = ""; for (let i = 0; i < 30 && !/Did not cross/.test(t2x); i++) { await L.sleep(1000); t2x = await L.text(p); } console.log("timeout shown after", Math.round((Date.now() - tr) / 1000), "s");
  const t2 = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "crossing test: with no crossing in 20 s it says \"Did not cross in 20 s\"", /Did not cross in 20 s/.test(t2), t2.slice(0, 300));
  await L.shot(p, F, "12-did-not-cross");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
