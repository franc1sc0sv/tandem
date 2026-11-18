import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F6-network";
try {
  const p = await L.openUi(m1, { hash: "#/desk" }); await L.sleep(1500); await p.locator("nav a, aside a", { hasText: "Red" }).first().click(); await L.sleep(2500);
  const before = (await L.text(p)).replace(/\n+/g, " | ");
  await p.getByRole("button", { name: "Medir width de band" }).click();
  const seen = [];
  for (let i = 0; i < 40; i++) { const t = (await L.text(p)).replace(/\n+/g, " | "); const h = await p.evaluate(() => location.hash); seen.push(h + " :: " + (t.match(/(\d+(\.\d+)?)\s*(Gbps|Mbps|Gb\/s|Mb\/s)[^|]*/)?.[0] ?? "")); await L.sleep(500); }
  const uniq = [...new Set(seen)]; console.log(uniq.slice(0, 8));
  await L.shot(p, F, "measure-end");
  const net = (await p.evaluate(() => document.body.innerText)).replace(/\n+/g, " | ");
  const m = net.match(/Ancho de band \| ([^|]*)\|/);
  const snap = await m1.snapshot();
  L.record(F, "\"Measure bandwidth\" gives a value", /(\d+(\.\d+)?)\s*(Gbps|Mbps|Gb\/s|Mb\/s|Gb|Mb)/.test(net), net.slice(0, 400));
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
