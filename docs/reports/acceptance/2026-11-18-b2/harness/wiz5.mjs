import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "win2"); const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Pasos] button", { hasText: s }).click();
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
  const cont = wiz(p).getByRole("button", { name: /Continuar/ });
  const enabled = await cont.isEnabled();
  L.record(F, "step 5: a real crossing passes the test and enables \"Continue\"", ok && /Crossed OK/.test(t5b) && enabled, `crossing=${ok} continue=${enabled}; ${t5b.slice(0, 300)}`);
  await L.shot(p, F, "11-step5-crossed");
  await step(p, "Done"); await L.sleep(800);
  const t6 = (await wiz(p).locator("main").innerText()).replace(/\n+/g, " | ");
  L.record(F, "summary: says \"crossing tested\" after the real crossing", /crossing tested/i.test(t6) && !/crossing not tested/.test(t6), t6.slice(0, 400));
  await L.shot(p, F, "12-summary");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
