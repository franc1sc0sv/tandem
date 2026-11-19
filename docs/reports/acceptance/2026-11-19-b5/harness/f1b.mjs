import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Steps] button", { hasText: s }).click();
try {
  const p1 = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1000);
  if ((await wiz(p1).count()) !== 1) await p1.getByRole("button", { name: /Open/ }).first().click();
  await L.until("setup m1", async () => (await wiz(p1).count()) === 1);
  for (const s of ["Keyboard and mouse", "Arrangement and crossing test", "Done"]) {
    await step(p1, s); await L.sleep(700);
    console.log("=== " + s); console.log((await wiz(p1).locator("main").innerText()).replace(/\n+/g, " | ").slice(0, 900));
  }
  await step(p1, "Arrangement and crossing test"); await L.sleep(500);
  const pc = wiz(p1).getByRole("button", { name: /Test crossing/ });
  console.log("test crossing btn:", await pc.count());
  await pc.click(); await L.sleep(1500);
  console.log("after test:", (await wiz(p1).locator("main").innerText()).replace(/\n+/g, " | ").slice(0, 700));
  await L.shot(p1, F, "05-m1-crossing-test");
  console.log(p1.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
