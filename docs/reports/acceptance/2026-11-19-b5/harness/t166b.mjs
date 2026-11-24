import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Steps] button", { hasText: s }).click();
const grp = (p, n) => wiz(p).locator(`[role=group][aria-label="${n}"]`).innerText();
let NA = "Mac", NB = "PC";
const dump = async (t, p1, p2) => console.log(t, JSON.stringify({ p1Mac: await grp(p1, NA), p1PC: await grp(p1, NB), p2Mac: await grp(p2, NA), p2PC: await grp(p2, NB) }).replace(/\\n/g, "|"));
try {
  { const s = await m1.snapshot(); NA = s.desk.machines[s.agent].name; NB = s.desk.machines[s.agent === "a" ? "b" : "a"].name; }
  const p1 = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1200); if ((await wiz(p1).count()) !== 1) await p1.getByRole("button", { name: /Open/ }).first().click();
  const p2 = await L.openUi(m2, { hash: "#/settings" }); await L.sleep(1200); if ((await wiz(p2).count()) !== 1) await p2.getByRole("button", { name: /Open/ }).first().click();
  for (const p of [p1, p2]) { await L.until("wiz", async () => (await wiz(p).count()) === 1); await step(p, "Keyboard and mouse"); }
  await L.sleep(1500);
  await dump("start", p1, p2);
  await p1.bringToFront(); await p1.keyboard.press("b"); await p1.mouse.move(300, 300); await p1.mouse.move(350, 380);
  await L.sleep(1500);
  await dump("after key in A", p1, p2);
  await p2.bringToFront(); await p2.keyboard.press("c"); await L.sleep(1500);
  await dump("after key in B", p1, p2);
  await L.shot(p1, "F1-setup", "t166-order-window-A"); await L.shot(p2, "F1-setup", "t166-order-window-B");
  console.log(await m1.snapshot().then((s) => JSON.stringify(s.input ?? s).slice(0, 300)));
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
