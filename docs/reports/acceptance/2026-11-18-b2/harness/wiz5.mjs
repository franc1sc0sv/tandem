import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "win2"); const F = "F1-asistente";
const wiz = (p) => p.locator('[role=dialog][aria-label=Asistente]');
const step = (p, s) => p.locator("aside[aria-label=Pasos] button", { hasText: s }).click();
async function entered(src, dst, [x, y, dx, dy], tries = 25) {
  for (let i = 0; i < tries; i++) {
    const before = (await dst.ctl({ cmd: "injected" })).count;
    await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90);
    const r = await dst.ctl({ cmd: "injected" });
    if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true;
  }
  return false;
}
try {
  const g = await L.geoLeftCenter(m1, m2);
  const p = await L.openUi(m1, { hash: "#/ajustes" }); await L.sleep(1000);
  await p.getByRole("button", { name: /Abrir/ }).first().click();
  await L.until("w", async () => (await wiz(p).count()) === 1);
  await step(p, "Acomodo y prueba de cruce"); await L.sleep(1200);
  const t5 = (await wiz(p).locator("main").innerText()).replace(/\n+/g, " | ");
  console.log("PASO5:", t5.slice(0, 900));
  L.record(F, "paso 5: el acomodo muestra resolución, proporción y escala reales de las pantallas", /1920/.test(t5) && /3440/.test(t5) && !/—/.test(t5.replace(/Tamaño[^|]*\|[^|]*—/, "")), t5.slice(0, 300));
  await L.shot(p, F, "10-paso5");
  await wiz(p).getByRole("button", { name: /Probar cruce/ }).click(); await L.sleep(1200);
  const ok = await entered(m1, m2, g.m1m2); await L.sleep(1800);
  const t5b = (await wiz(p).locator("main").innerText()).replace(/\n+/g, " | ");
  const cont = wiz(p).getByRole("button", { name: /Continuar/ });
  const enabled = await cont.isEnabled();
  L.record(F, "paso 5: un cruce real pasa la prueba y habilita «Continuar»", ok && /Cruzó bien/.test(t5b) && enabled, `cruce=${ok} continuar=${enabled}; ${t5b.slice(0, 300)}`);
  await L.shot(p, F, "11-paso5-cruzo");
  await step(p, "Listo"); await L.sleep(800);
  const t6 = (await wiz(p).locator("main").innerText()).replace(/\n+/g, " | ");
  L.record(F, "resumen: dice «cruce probado» tras el cruce real", /cruce probado/i.test(t6) && !/cruce sin probar/.test(t6), t6.slice(0, 400));
  await L.shot(p, F, "12-resumen");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
