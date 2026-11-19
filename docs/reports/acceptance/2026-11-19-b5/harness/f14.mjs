import * as L from "./lib.mjs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F14-tray";
try {
  const ctx = await (await L.getBrowser()).newContext({ viewport: { width: 380, height: 600 }, locale: "en-US", colorScheme: "dark" });
  await ctx.addInitScript(L.INIT); const p = await ctx.newPage(); p.setDefaultTimeout(5000); const errs = []; p.on("console", (m) => m.type() === "error" && errs.push(m.text())); p.on("pageerror", (e) => errs.push(e.message));
  await p.goto(`${m1.origin}/tray.html`); await L.sleep(2000);
  const t = (await L.text(p)).replace(/\n+/g, " | "); console.log("BANDEJA:", t);
  console.log("controls:", await p.locator("button,a").allInnerTexts());
  await L.shot(p, F, "01-tray");
  L.record(F, "the tray mini scene shows the desk state", /(Mac|PC|Simultaneo)/.test(t) && !/undefined|NaN/.test(t), t.slice(0, 250));
  // apply a scene from the tray
  const sceneBtn = p.locator("button", { hasText: /^Solo Std$/ }).first();
  const before = (await m1.snapshot()).state.scene;
  if (await sceneBtn.count()) { const label = await sceneBtn.innerText(); await sceneBtn.click(); await L.sleep(4000); const after = (await m1.snapshot()).state.scene; L.record(F, "applying a scene from the tray applies it in the agent", after !== before, `${label.replace(/\n/g, " ")}: ${before} → ${after}`); }
  else L.record(F, "applying a scene from the tray", false, "there are no scene buttons in the tray");
  await p.getByRole("button", { name: "Open" }).click(); await L.sleep(500);
  const last = p.locator("button").last(); const lastInfo = await last.evaluate((e) => ({ aria: e.getAttribute("aria-label"), title: e.getAttribute("title"), html: e.outerHTML.slice(0, 160) })); console.log("last button:", lastInfo); await last.click(); await L.sleep(500);
  const calls = await m1.calls();
  L.record(F, "\"Open Tandem\" and \"Quit\" in the tray reach the app core (ui_show_main / ui_quit)", calls.some((c) => c.call === "ui_show_main") && calls.some((c) => c.call === "ui_quit"), JSON.stringify(calls.map((c) => c.call)));
  console.log(errs); await ctx.close(); await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
