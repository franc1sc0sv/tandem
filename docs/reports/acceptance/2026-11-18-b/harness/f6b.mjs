import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { writeFileSync, rmSync } from "node:fs";
const tag = process.argv[2] ?? "host";
const { m1, m2 } = L.attach(tag);
const F = "F6-red";
const cut = (k) => writeFileSync(`${L.RUN}/${tag}/cut-${k}`, ""), heal = (k) => rmSync(`${L.RUN}/${tag}/cut-${k}`, { force: true });
const link = async () => (await m1.snapshot()).peer.link ?? (await m1.snapshot()).peer.kind;
try {
  const p = await L.openUi(m1, { hash: "#/puesto" }); await L.sleep(1500); await p.locator("nav a, aside a", { hasText: "Red" }).first().click(); await L.sleep(3000);
  const t0txt = (await L.text(p)).replace(/\n+/g, " | ");
  console.log("antes:", t0txt.slice(100, 700));
  L.record(F, "con dos caminos: USB-C principal y Wi-Fi de respaldo", /Principal/.test(t0txt) && /Respaldo|respaldo/.test(t0txt), t0txt.slice(300, 650));
  await L.shot(p, F, "01-dos-caminos");
  // Cortar el principal.
  const t = Date.now(); cut("usb4");
  let took = null; const polled = [];
  while (Date.now() - t < 15000) { const l = await link(); polled.push(l); if (l === "ethernet" || l === "wifi") { took = Date.now() - t; break; } await L.sleep(40); }
  L.record(F, "cortar el camino principal: la entrada pasa al siguiente en < 1 s", took !== null && took < 1000, took === null ? "no pasó en 15 s" : `${took} ms`);
  await L.sleep(2500);
  const t1 = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "02-tras-corte");
  L.record(F, "el evento del cambio de camino es visible en Red", /(pasó a|Se desconectó)/i.test(t1), t1.slice(100, 900));
  // Medir por el camino que lleva la entrada.
  const btn = p.getByRole("button", { name: /Medir ancho de banda/ });
  const dis = await btn.isDisabled(); if (!dis) { await btn.click(); await L.sleep(6000); }
  const t2 = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "tras el corte «Medir ancho de banda» mide el camino que lleva la entrada (o explica por qué no)", /(\d+)\s*(Mbps|Gbps)/.test(t2) || /No se mide/.test(t2), t2.slice(300, 700));
  heal("usb4"); await L.sleep(8000);
  const back = await link();
  L.record(F, "al volver el cable, el camino principal retoma la entrada", back === "usb4", `camino ahora: ${back}`);
  await L.shot(p, F, "03-recuperado");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
