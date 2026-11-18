import * as L from "./lib.mjs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F14-bandeja";
try {
  const ctx = await (await L.getBrowser()).newContext({ viewport: { width: 380, height: 600 }, locale: "es-ES", colorScheme: "dark" });
  await ctx.addInitScript(L.INIT); const p = await ctx.newPage(); p.setDefaultTimeout(5000); const errs = []; p.on("console", (m) => m.type() === "error" && errs.push(m.text())); p.on("pageerror", (e) => errs.push(e.message));
  await p.goto(`${m1.origin}/tray.html`); await L.sleep(2000);
  const t = (await L.text(p)).replace(/\n+/g, " | "); console.log("BANDEJA:", t);
  console.log("controles:", await p.locator("button,a").allInnerTexts());
  await L.shot(p, F, "01-bandeja");
  L.record(F, "la mini escena de la bandeja muestra el estado del puesto", /(Mac|PC|Simultaneo)/.test(t) && !/undefined|NaN/.test(t), t.slice(0, 250));
  // aplicar una escena desde la bandeja
  const sceneBtn = p.locator("button", { hasText: /^Std a PC$|^Solo Std$/ }).first();
  const before = (await m1.snapshot()).state.scene;
  if (await sceneBtn.count()) { const label = await sceneBtn.innerText(); await sceneBtn.click(); await L.sleep(4000); const after = (await m1.snapshot()).state.scene; L.record(F, "aplicar una escena desde la bandeja la aplica en el agente", after !== before, `${label.replace(/\n/g, " ")}: ${before} → ${after}`); }
  else L.record(F, "aplicar una escena desde la bandeja", false, "no hay botones de escena en la bandeja");
  await p.getByRole("button", { name: "Abrir" }).click(); await L.sleep(500);
  const last = p.locator("button").last(); const lastInfo = await last.evaluate((e) => ({ aria: e.getAttribute("aria-label"), title: e.getAttribute("title"), html: e.outerHTML.slice(0, 160) })); console.log("último botón:", lastInfo); await last.click(); await L.sleep(500);
  const calls = await m1.calls();
  L.record(F, "«Abrir Tandem» y «Salir» de la bandeja llegan al núcleo de la app (ui_show_main / ui_quit)", calls.some((c) => c.call === "ui_show_main") && calls.some((c) => c.call === "ui_quit"), JSON.stringify(calls.map((c) => c.call)));
  console.log(errs); await ctx.close(); await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
