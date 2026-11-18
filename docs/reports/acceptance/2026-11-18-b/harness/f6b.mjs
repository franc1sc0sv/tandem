import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { writeFileSync, rmSync } from "node:fs";
const tag = process.argv[2] ?? "host";
const { m1, m2 } = L.attach(tag);
const F = "F6-network";
const cut = (k) => writeFileSync(`${L.RUN}/${tag}/cut-${k}`, ""), heal = (k) => rmSync(`${L.RUN}/${tag}/cut-${k}`, { force: true });
const link = async () => (await m1.snapshot()).peer.link ?? (await m1.snapshot()).peer.kind;
try {
  const p = await L.openUi(m1, { hash: "#/desk" }); await L.sleep(1500); await p.locator("nav a, aside a", { hasText: "Red" }).first().click(); await L.sleep(3000);
  const t0txt = (await L.text(p)).replace(/\n+/g, " | ");
  console.log("before:", t0txt.slice(100, 700));
  L.record(F, "with two paths: primary USB-C and backup Wi-Fi", /Principal/.test(t0txt) && /Respaldo|respaldo/.test(t0txt), t0txt.slice(300, 650));
  await L.shot(p, F, "01-two-paths");
  // Cut the primary.
  const t = Date.now(); cut("usb4");
  let took = null; const polled = [];
  while (Date.now() - t < 15000) { const l = await link(); polled.push(l); if (l === "ethernet" || l === "wifi") { took = Date.now() - t; break; } await L.sleep(40); }
  L.record(F, "cutting the primary path: input moves to the next one in < 1 s", took !== null && took < 1000, took === null ? "did not happen in 15 s" : `${took} ms`);
  await L.sleep(2500);
  const t1 = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "02-after-outage");
  L.record(F, "the path-change event is visible in Network", /(Switched to|unplugged)/i.test(t1), t1.slice(100, 900));
  // Measure through the path carrying input.
  const btn = p.getByRole("button", { name: /Medir width de band/ });
  const dis = await btn.isDisabled(); if (!dis) { await btn.click(); await L.sleep(6000); }
  const t2 = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "after the cut \"Measure bandwidth\" measures the path carrying input (or explains why not)", /(\d+)\s*(Mbps|Gbps)/.test(t2) || /Not measured/.test(t2), t2.slice(300, 700));
  heal("usb4"); await L.sleep(8000);
  const back = await link();
  L.record(F, "when the cable returns, the primary path takes input back", back === "usb4", `path now: ${back}`);
  await L.shot(p, F, "03-recovered");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
