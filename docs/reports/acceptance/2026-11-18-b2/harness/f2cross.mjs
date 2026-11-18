import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "fin");
const F = "F2-prueba-de-cruce-en-vivo";
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
  // 1) cruce bueno: por el borde del acomodo
  const p = await L.openUi(m1, { hash: "#/acomodo" }); await L.sleep(1200);
  const btn = p.locator("a:has-text('Probar cruce'), button:has-text('Probar cruce')").first();
  const disabled = await btn.evaluate((e) => e.disabled === true || e.getAttribute("aria-disabled") === "true");
  L.record(F, "Acomodo: «Probar cruce» está habilitado con el agente real", !disabled);
  await btn.click(); await L.sleep(1200);
  const t0 = (await L.text(p)).replace(/\n+/g, " | ");
  console.log("esperando:", t0.slice(0, 400));
  await L.shot(p, F, "10-esperando");
  const ok = await entered(m1, m2, g.m1m2);
  await L.sleep(1800);
  const t1 = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "prueba de cruce: un cruce real del motor por el borde del acomodo da «Cruzó bien»", ok && /Cruzó bien/.test(t1), `cruce=${ok}; ${t1.slice(0, 400)}`);
  await L.shot(p, F, "11-cruzo-bien");
  // 2) sin cruce: 20 s
  await p.goto(m1.origin + "/#/acomodo/cruce"); await p.reload(); await L.sleep(21500);
  const t2 = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "prueba de cruce: sin cruce en 20 s dice «No cruzó en 20 s»", /No cruzó en 20 s/.test(t2), t2.slice(0, 300));
  await L.shot(p, F, "12-no-cruzo");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
