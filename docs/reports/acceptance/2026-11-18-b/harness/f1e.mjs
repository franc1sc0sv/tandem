import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "fin"); const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
try {
  const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1000); await p.getByRole("button", { name: /Open/ }).first().click(); await L.sleep(600);
  await p.locator("aside[aria-label=Pasos] button", { hasText: "Keyboard and mouse" }).click(); await L.sleep(1500);
  const hub = [(await m1.snapshot()).input.hub_here, (await m2.snapshot()).input.hub_here];
  const rows = await wiz(p).locator(".wz-mc").evaluateAll((els) => els.map((e) => e.innerText.replace(/\n+/g, " | ")));
  const wrong = hub.map((h, i) => ({ h, row: rows[i] })).filter((x) => x.h !== true && /Keyboard and mouse/.test(x.row ?? ""));
  L.record(F, "step 4: a machine without a keyboard or mouse connected does not appear with \"Keyboard and mouse\" detected", wrong.length === 0, `hub_here m1/m2 = ${hub}; UI rows: ${JSON.stringify(rows)}`);
  await L.shot(p, F, "04-keyboard-and-mouse");
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
