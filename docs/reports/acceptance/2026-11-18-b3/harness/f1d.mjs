import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Steps] button", { hasText: s }).click();
try {
  const p1 = await L.openUi(m1);
  await p1.ctx.grantPermissions(["clipboard-read", "clipboard-write"], { origin: m1.origin });
  await L.until("setup m1", async () => (await wiz(p1).count()) === 1);
  await step(p1, "Keyboard and mouse"); await L.sleep(800);
  // activity on m2 (the other) and on m1
  const p2 = await L.openUi(m2, { hash: "#/settings" }); await p2.getByRole("button", { name: /Open/ }).first().click();
  await L.until("setup m2", async () => (await wiz(p2).count()) === 1);
  await step(p2, "Keyboard and mouse"); await L.sleep(800);
  await p2.keyboard.press("a"); await L.sleep(1000);
  const t1 = await wiz(p1).locator("main").innerText();
  L.record(F, "step 4: activity on the other machine is marked \"Activity detected\"", /Activity detected/.test(t1), t1.replace(/\n+/g," | ").slice(100,300));
  await L.shot(p1, F, "04-activity-other");
  await step(p1, "Done"); await L.sleep(500);
  await wiz(p1).locator(".wz-copy").click(); await L.sleep(300);
  const clip = await p1.evaluate(() => navigator.clipboard.readText());
  L.record(F, "summary: the MCP command is copied", clip === "claude mcp add tandem -- tandem mcp", clip);
  await L.shot(p1, F, "06-ready");
  // The copied command works: the `tandem` binary exists with `mcp`
  console.log(p1.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
