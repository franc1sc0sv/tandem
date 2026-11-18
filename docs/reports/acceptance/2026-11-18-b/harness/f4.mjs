import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F4-monitores";
const owner = async (m, name) => (await m.snapshot()).monitors.find((x) => x.name === name)?.owner;
try {
  const p = await L.openUi(m1, { hash: "#/monitores" }); await L.sleep(1500);
  const txt = (await L.text(p));
  L.record(F, "monitor de fábrica se muestra «No compatible» con su razón", /Factory-32[\s\S]*No compatible[\s\S]*Firmware sin el estándar/.test(txt));
  // Sin escrituras: se limpian los registros y se provoca todo lo que la UI permite sobre monitores.
  await m2.ctl({ cmd: "monitor_clear_ops" }); await m1.ctl({ cmd: "monitor_clear_ops" });
  const opts = await p.locator("article", { hasText: "Factory-32" }).locator("[role=radio], button").allInnerTexts();
  console.log("controles de la tarjeta de fábrica:", opts);
  await p.getByRole("button", { name: /Qué es el estándar/ }).click(); await L.sleep(500);
  console.log("«Qué es el estándar»:", (await L.text(p)).replace(/\n+/g, " | ").slice(-400));
  await L.sleep(6000);
  const ops = [...(await m2.ctl({ cmd: "monitor_ops" })).ops, ...(await m1.ctl({ cmd: "monitor_ops" })).ops];
  const factoryHandles = new Set(); 
  const writes = ops.filter((o) => o.startsWith("Set"));
  L.record(F, "monitor de fábrica: ninguna escritura DDC en el bus", !ops.some((o) => o.startsWith("Set") && /MonitorHandle\((2)\)/.test(o)), `Set vistos: ${writes.length}; total ops ${ops.length}`);
  // Re-elección de dueño al apagar el dueño.
  const o1 = await owner(m1, "Std-34"); console.log("dueño actual:", o1);
  const ownerM = (await m1.snapshot()).agent === o1 ? m1 : m2, other = ownerM === m1 ? m2 : m1;
  ownerM.kill("SIGKILL"); await L.sleep(500);
  const pOther = await L.openUi(other, { hash: "#/monitores" });
  let newOwner = null;
  try { newOwner = await L.until("nuevo dueño", async () => { const o = await owner(other, "Std-34"); return o && o !== o1 ? o : null; }, 25000, 500); } catch {}
  await L.sleep(500);
  const txt2 = (await L.text(pOther)).replace(/\n+/g, " | ");
  L.record(F, "se apaga el dueño del monitor: el otro agente se vuelve dueño", newOwner === (await other.snapshot()).agent, `dueño antes ${o1}, después ${newOwner}`);
  await L.shot(pOther, F, "reeleccion-dueno");
  console.log(txt2.slice(0, 400));
  // vuelve el dueño
  ownerM.startAgent(); await L.sleep(8000);
  const info = JSON.parse(require("node:fs").readFileSync(`${L.RUN}/${process.argv[2] ?? "host"}/info.json`, "utf8")); info[ownerM === m1 ? "m1" : "m2"].pid = ownerM.pid; require("node:fs").writeFileSync(`${L.RUN}/${process.argv[2] ?? "host"}/info.json`, JSON.stringify(info));
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
