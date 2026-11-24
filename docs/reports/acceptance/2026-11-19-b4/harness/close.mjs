import * as L from "./lib.mjs";
const { m1 } = L.attach(process.argv[2] ?? "c2"); const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
try {
  const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1000);
  await p.getByRole("button", { name: /Open/ }).first().click(); await L.sleep(1500);
  const b = await p.locator(".wz-close").boundingBox();
  const hit = await p.evaluate(() => { const c = document.querySelector(".wz-close"); const e = c.getBoundingClientRect(); const t = document.elementFromPoint(e.x + e.width / 2, e.y + e.height / 2); return c.contains(t); });
  await L.shot(p, F, "00-close");
  await p.locator(".wz-close").click(); await L.sleep(800);
  const closed = (await wiz(p).count()) === 0;
  L.record(F, "the setup \"Close\" button is visible, receives the click and closes setup", b.height >= 24 && hit && closed, `box ${Math.round(b.width)}×${Math.round(b.height)} px; receives the click: ${hit}; closed: ${closed}`);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
