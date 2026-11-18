import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F1-asistente";
const wiz = (p) => p.locator('[role=dialog][aria-label=Asistente]');
const step = (p, s) => p.locator("aside[aria-label=Pasos] button", { hasText: s }).click();
try {
  const p1 = await L.openUi(m1);
  await p1.ctx.grantPermissions(["clipboard-read", "clipboard-write"], { origin: m1.origin });
  await L.until("asistente m1", async () => (await wiz(p1).count()) === 1);
  await step(p1, "Teclado y mouse"); await L.sleep(800);
  // actividad en m2 (la otra) y en m1
  const p2 = await L.openUi(m2, { hash: "#/ajustes" }); await p2.getByRole("button", { name: /Abrir/ }).first().click();
  await L.until("asistente m2", async () => (await wiz(p2).count()) === 1);
  await step(p2, "Teclado y mouse"); await L.sleep(800);
  await p2.keyboard.press("a"); await L.sleep(1000);
  const t1 = await wiz(p1).locator("main").innerText();
  L.record(F, "paso 4: actividad en la otra máquina se marca «Actividad detectada»", /Actividad detectada/.test(t1), t1.replace(/\n+/g," | ").slice(100,300));
  await L.shot(p1, F, "04-actividad-otra");
  await step(p1, "Listo"); await L.sleep(500);
  await wiz(p1).locator(".wz-copy").click(); await L.sleep(300);
  const clip = await p1.evaluate(() => navigator.clipboard.readText());
  L.record(F, "resumen: el comando MCP se copia", clip === "claude mcp add tandem -- tandem mcp", clip);
  await L.shot(p1, F, "06-listo");
  // El comando copiado funciona: el binario `tandem` existe con `mcp`
  console.log(p1.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
