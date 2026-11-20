import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Steps] button", { hasText: s }).click();
try {
  await m1.ctl({ cmd: "devices", list: [[0x046d, 0xc33f], [0x046d, 0xc539]] });
  const p1 = await L.openUi(m1);
  // Round 6: on a Windows m1 setup does not open by itself (no tour): open it from Settings.
  await L.sleep(1500); if ((await wiz(p1).count()) !== 1) { await p1.goto(m1.origin + "/#/settings"); await L.sleep(1200); await p1.getByRole("button", { name: /Open/ }).first().click({ force: true }); }
  await L.until("setup m1", async () => (await wiz(p1).count()) === 1);
  await step(p1, "Keyboard and mouse"); await L.sleep(1500);
  console.log("=== K&M m1:", (await wiz(p1).locator("main").innerText()).replace(/\n+/g, " | "));
  await L.shot(p1, F, "04-m1-keyboard-detected");
  const snap = await m1.snapshot(); console.log(JSON.stringify(snap.input).slice(0,400)); console.log(JSON.stringify(snap.caps.here.input), JSON.stringify(snap.caps.here.hub));
  await step(p1, "Permissions"); await L.sleep(500); console.log("=== permissions:", (await wiz(p1).locator("main").innerText()).replace(/\n+/g, " | ").slice(0,300));
  await step(p1, "Done"); await L.sleep(500); 
  console.log("=== ready:", (await wiz(p1).locator("main").innerText()).replace(/\n+/g, " | "));
  console.log("buttons:", await wiz(p1).locator("main button").allInnerTexts());
  console.log(p1.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
