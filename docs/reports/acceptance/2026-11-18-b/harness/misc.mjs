import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { readFileSync, existsSync } from "node:fs";
const { m1 } = L.attach(process.argv[2] ?? "fin"); const D = "http://127.0.0.1:38123";
// (a) atajo de la ventana
{ const F = "F12-ajustes-paleta"; const p = await L.openUi(m1, { hash: "#/ajustes" }); await L.sleep(1000); await p.getByRole("tab", { name: "Atajos" }).click(); await L.sleep(400);
  await p.getByRole("button", { name: /Mostrar u ocultar Tandem/ }).click(); await L.sleep(200); await p.keyboard.press("Control+Alt+8"); await L.sleep(1200);
  const f = `${m1.cfg}/Tandem/ui.toml`; const txt = existsSync(f) ? readFileSync(f, "utf8") : "(sin ui.toml)";
  L.record(F, "Atajos: cambiar «Mostrar u ocultar Tandem» se guarda en ui.toml", /Alt\+Digit8|Ctrl\+Alt\+Digit8/.test(txt), txt.replace(/\n/g, " "));
  // el atajo local de la ventana lo atiende cuando el sistema no deja registrarlo
  await p.ctx.close(); }
// (d) prueba de cruce en demo: los 4 estados
{ const F = "P6-prueba-de-cruce-demo"; for (const [q, re, name] of [["?demo&cross=ok", /(Cruzó|bien|pasó)/i, "cruzó bien"], ["?demo&cross=wrong", /(equivocad|borde)/i, "borde equivocado"], ["?demo&cross=timeout", /(tiempo|agotad|no cruz)/i, "tiempo agotado"], ["?demo", /(Esperando|Lleva el mouse|cruz)/i, "esperando"]]) {
  const p = await L.openDemo(D, q, "#/acomodo/cruce"); await L.sleep(q.includes("timeout") ? 14000 : 2500); const t = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, `demo: estado «${name}» de la prueba de cruce`, re.test(t) && !/undefined|NaN/.test(t) && p.errors.length === 0, t.slice(60, 260)); await p.ctx.close(); } }
// (e) movimiento reducido
{ const F = "G8-reduced-motion"; const bad = [];
  for (const [q, h] of [["?demo", "#/puesto"], ["?demo", "#/monitores"], ["?demo", "#/tuneles"], ["?demo", "#/red"], ["?demo&wizard=1", ""], ["?demo&txn=run", "#/progreso"]]) {
    const p = await L.openDemo(D, q, h, { width: 1280, height: 860 }, "reduce"); await L.sleep(1500);
    const anims = await p.evaluate(() => [...document.querySelectorAll("*")].filter((e) => { const c = getComputedStyle(e); return (c.animationName !== "none" && parseFloat(c.animationDuration) > 0.01) || (c.transitionProperty !== "all" && c.transitionProperty !== "none" && parseFloat(c.transitionDuration) > 0.01) || (c.transitionProperty === "all" && parseFloat(c.transitionDuration) > 0.01); }).slice(0, 4).map((e) => e.tagName + "." + String(e.className).slice(0, 40) + ":" + getComputedStyle(e).animationName + "/" + getComputedStyle(e).transitionDuration));
    if (anims.length) bad.push(`${q}${h}: ${anims.join(", ")}`); await p.ctx.close(); }
  L.record(F, "con prefers-reduced-motion: reduce no hay animaciones ni transiciones", bad.length === 0, bad.join(" ; ").slice(0, 500)); }
// (g7) con movimiento normal sí hay animación
{ const F = "G7-animaciones"; const p = await L.openDemo(D, "?demo", "#/puesto"); await L.sleep(400);
  const n = await p.evaluate(() => [...document.querySelectorAll("*")].filter((e) => { const c = getComputedStyle(e); return c.animationName !== "none" || parseFloat(c.transitionDuration) > 0; }).length);
  L.record(F, "con movimiento normal hay entradas/transiciones animadas en el Puesto", n > 5, `${n} elementos animados`); await p.ctx.close(); }
await L.closeBrowser();
