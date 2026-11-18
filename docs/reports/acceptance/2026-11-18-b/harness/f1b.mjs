import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "main");
const F = "F1-asistente";
const wiz = (p) => p.locator('[role=dialog][aria-label=Asistente]');
const step = (p, s) => p.locator("aside[aria-label=Pasos] button", { hasText: s }).click();
try {
  const p1 = await L.openUi(m1, { hash: "#/ajustes" }); await L.sleep(1000);
  if ((await wiz(p1).count()) !== 1) await p1.getByRole("button", { name: /Abrir/ }).first().click();
  await L.until("asistente m1", async () => (await wiz(p1).count()) === 1);
  for (const s of ["Teclado y mouse", "Acomodo y prueba de cruce", "Listo"]) {
    await step(p1, s); await L.sleep(700);
    console.log("=== " + s); console.log((await wiz(p1).locator("main").innerText()).replace(/\n+/g, " | ").slice(0, 900));
  }
  await step(p1, "Acomodo y prueba de cruce"); await L.sleep(500);
  const pc = wiz(p1).getByRole("button", { name: /Probar cruce/ });
  console.log("probar cruce btn:", await pc.count());
  await pc.click(); await L.sleep(1500);
  console.log("after probar:", (await wiz(p1).locator("main").innerText()).replace(/\n+/g, " | ").slice(0, 700));
  await L.shot(p1, F, "05-m1-prueba-cruce");
  console.log(p1.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
