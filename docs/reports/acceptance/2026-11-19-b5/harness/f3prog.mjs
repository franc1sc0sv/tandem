// F3 "Save and apply": every hash the window goes through (hashchange listener) and the
// transaction events the store receives, to see whether the progress screen is shown.
import * as L from "./lib.mjs";
const tag = process.argv[2] ?? "ow"; const { m1, m2 } = L.attach(tag);
const F = "F3-scenes";
const pickRadio = (p, label, opt) => p.locator(".f", { hasText: `${label} shows` }).locator("[role=radio]", { hasText: opt }).click();
try {
  const ctx = await (await L.getBrowser()).newContext({ viewport: { width: 1280, height: 860 }, locale: "en-US", colorScheme: "dark" });
  await ctx.addInitScript(() => { window.__log = []; const t0 = performance.now(); const T = () => Math.round(performance.now() - t0);
    const of = window.fetch; window.fetch = async (u, o) => { const b = o?.body ?? ""; const tag = String(u).replace("/__bridge/invoke/", "") + " " + String(b).slice(0, 60); if (/invoke/.test(String(u))) window.__log.push([T(), "call", tag]); const r = await of(u, o); if (/invoke/.test(String(u))) window.__log.push([T(), "reply", tag]); return r; };
    const OE = window.EventSource; window.EventSource = function (u) { const es = new OE(u); es.addEventListener("message", (e) => { const d = JSON.parse(e.data); if (/notice/.test(d.event) && /txn/.test(e.data)) window.__log.push([T(), "event", d.event + " " + JSON.stringify(d.payload).slice(0, 60)]); }); return es; };
    addEventListener("hashchange", () => window.__log.push([T(), "hash", location.hash])); });
  await ctx.addInitScript(L.INIT);
  const p = await ctx.newPage(); p.errors = []; await p.goto(m1.origin + "/#/monitors/scene"); await L.sleep(2000);
  await p.evaluate(() => { window.__h = [[0, location.hash]]; const t0 = performance.now(); addEventListener("hashchange", () => window.__h.push([Math.round(performance.now() - t0), location.hash])); });
  const name = "Prog " + (process.argv[3] ?? "A");
  await p.locator("input").first().fill(name);
  await pickRadio(p, "Std-34", process.argv[4] ?? "PC");
  const before = await m1.calls();
  await p.getByRole("button", { name: "Save and apply" }).click();
  const texts = [];
  for (let i = 0; i < 30; i++) { const t = (await L.text(p)).replace(/\n+/g, " | "); if (/Changes in progress/.test(t)) texts.push(t.slice(0, 300)); await L.sleep(100); }
  await L.sleep(1500);
  const h = await p.evaluate(() => window.__h);
  console.log("hashes:", JSON.stringify(h)); console.log((await p.evaluate(() => window.__log)).filter((x) => !/snapshot|resources/.test(x[2])).map((x) => x.join(" ")).join("\n"));
  console.log("progress texts seen:", texts.length, texts[0] ?? "");
  await L.shot(p, F, "prog-after-save-and-apply");
  const toasts = await p.locator("[role=status], [role=alert]").allInnerTexts().catch(() => []);
  console.log("toasts:", JSON.stringify(toasts));
  L.record(F, "Save and apply opens Changes in progress (F-Transactions)", h.some(([, x]) => x === "#/progress") && texts.length > 0, JSON.stringify(h) + " texts=" + texts.length);
  console.log(p.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
