import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Steps] button", { hasText: s }).click();
const grp = (p, n) => wiz(p).locator(`[role=group][aria-label="${n}"]`).innerText();
try {
  const nm = async (m) => { const s = await m.snapshot(); return s.desk.machines[s.agent].name; }; const N1 = await nm(m1), N2 = await nm(m2);
  const p1 = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1200); if ((await wiz(p1).count()) !== 1) await p1.getByRole("button", { name: /Open/ }).first().click();
  const p2 = await L.openUi(m2, { hash: "#/settings" }); await L.sleep(1200); if ((await wiz(p2).count()) !== 1) await p2.getByRole("button", { name: /Open/ }).first().click();
  for (const p of [p1, p2]) { await L.until("wiz", async () => (await wiz(p).count()) === 1); await step(p, "Keyboard and mouse"); }
  await L.sleep(1500);
  const has = (t) => /Activity detected/.test(t);
  L.record(F, "T-166 baseline: no row shows activity before pressing", !has(await grp(p1, N1)) && !has(await grp(p1, N2)) && !has(await grp(p2, N1)) && !has(await grp(p2, N2)));
  await p2.keyboard.press("a"); await p2.mouse.move(100, 100); await p2.mouse.move(200, 220);
  await L.until("A sees B", async () => has(await grp(p1, N2)), 8000).then(() => L.record(F, "T-166: activity on B (key in B's window) marks B's row in A's window", true), (e) => L.record(F, "T-166: activity on B marks B's row in A's window", false, e.message));
  L.record(F, "T-166: A's own row in A's window stays unmarked", !has(await grp(p1, N1)), await grp(p1, N1));
  await L.shot(p1, F, "t166-a-sees-b");
  await p1.keyboard.press("b");
  await L.until("B sees A", async () => has(await grp(p2, N1)), 8000).then(() => L.record(F, "T-166 vice versa: activity on A marks A's row in B's window", true), (e) => L.record(F, "T-166 vice versa fails", false, e.message));
  await L.shot(p2, F, "t166-b-sees-a");
  console.log(p1.errors, p2.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
