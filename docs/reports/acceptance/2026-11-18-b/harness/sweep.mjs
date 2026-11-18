// Barrido: cada botón visible de cada pantalla (agente real) tiene que producir algún efecto observable.
import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { readFileSync } from "node:fs"; import { createHash } from "node:crypto";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "G6-botones-con-efecto";
const SKIP = /^(Salir|Confirmar|Desemparejar|Apagar|Cerrar)$/;
const logLines = (m) => readFileSync(m.log, "utf8").split("\n").length;
const routes = ["#/puesto", "#/tuneles", "#/monitores", "#/acomodo", "#/red", "#/maquina/a", "#/maquina/b", "#/ajustes", "#/ajustes:Atajos", "#/ajustes:Teclado y mouse", "#/ajustes:Actualizaciones", "#/ajustes:Avanzado", "#/estados"];
const noEffect = [], tried = [];
for (const r of routes) {
  const [hash, tab] = r.split(":");
  const p = await L.openUi(m1, { hash }); await L.sleep(1500);
  if (tab) { await p.getByRole("tab", { name: tab }).click(); await L.sleep(400); }
  const labels = await p.locator("main button:visible, main a[role=button]:visible").evaluateAll((els) => els.map((e, i) => ({ i, t: ((e.getAttribute("aria-label") || e.innerText || "") + "").trim().replace(/\n/g, " ") })));
  for (const [idx, b] of labels.entries()) {
    if (SKIP.test(b.t) || !b.t) continue;
    await p.goto(m1.origin + "/" + hash); await L.sleep(900); if (tab) { await p.getByRole("tab", { name: tab }).click(); await L.sleep(300); }
    const el = p.locator("main button:visible, main a[role=button]:visible").nth(idx);
    if ((await el.count()) === 0) continue;
    const dis = await el.isDisabled().catch(() => false);
    const snap = async () => JSON.stringify([createHash("md5").update(await L.text(p)).digest("hex"), await p.evaluate(() => location.hash), await p.locator("[role=dialog]").count(), logLines(m1) + logLines(m2), (await m1.calls()).length, await p.evaluate(() => document.querySelectorAll("[aria-checked=true],[aria-selected=true],[aria-pressed=true]").length + "|" + [...document.querySelectorAll("[aria-checked],[aria-selected],[aria-pressed]")].map(e => e.getAttribute("aria-checked") + e.getAttribute("aria-selected") + e.getAttribute("aria-pressed")).join(""))]);
    const before = await snap();
    try { await el.click({ timeout: 3000, force: true }); } catch (e) { tried.push(`${r} «${b.t}»: no se pudo pulsar (${e.message.split("\n")[0]})`); continue; }
    await L.sleep(900); const after = await snap();
    tried.push(`${r} «${b.t}»${dis ? " [deshabilitado]" : ""}: ${before === after ? "SIN EFECTO" : "ok"}`);
    if (before === after) noEffect.push(`${r} «${b.t}»${dis ? " (deshabilitado)" : ""}`);
  }
  await p.ctx.close();
}
console.log(tried.join("\n"));
console.log("SIN EFECTO:", JSON.stringify(noEffect, null, 1));
L.record(F, "cada botón visible de las pantallas en vivo produce un efecto observable", noEffect.filter((x) => !/deshabilitado/.test(x)).length === 0, noEffect.join(" ; "));
await L.closeBrowser();
