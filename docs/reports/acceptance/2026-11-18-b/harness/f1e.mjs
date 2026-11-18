import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "fin"); const F = "F1-asistente";
const wiz = (p) => p.locator('[role=dialog][aria-label=Asistente]');
try {
  const p = await L.openUi(m1, { hash: "#/ajustes" }); await L.sleep(1000); await p.getByRole("button", { name: /Abrir/ }).first().click(); await L.sleep(600);
  await p.locator("aside[aria-label=Pasos] button", { hasText: "Teclado y mouse" }).click(); await L.sleep(1500);
  const hub = [(await m1.snapshot()).input.hub_here, (await m2.snapshot()).input.hub_here];
  const rows = await wiz(p).locator(".wz-mc").evaluateAll((els) => els.map((e) => e.innerText.replace(/\n+/g, " | ")));
  const wrong = hub.map((h, i) => ({ h, row: rows[i] })).filter((x) => x.h !== true && /Teclado y mouse/.test(x.row ?? ""));
  L.record(F, "paso 4: una máquina sin teclado ni mouse conectados no aparece con «Teclado y mouse» detectado", wrong.length === 0, `hub_here m1/m2 = ${hub}; filas de la UI: ${JSON.stringify(rows)}`);
  await L.shot(p, F, "04-teclado-y-mouse");
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
