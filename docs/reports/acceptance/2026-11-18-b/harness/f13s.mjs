import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { writeFileSync, rmSync, readFileSync } from "node:fs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F13-estados-dificiles";
const info = (who, m) => { const i = JSON.parse(readFileSync(`${L.RUN}/${tag}/info.json`, "utf8")); i[who].pid = m.pid; writeFileSync(`${L.RUN}/${tag}/info.json`, JSON.stringify(i)); };
const body = async (p) => (await L.text(p)).replace(/\n+/g, " | ");
const btns = (p) => p.locator("main button").allInnerTexts();
try {
  const p = await L.openUi(m1, { hash: "#/puesto" }); await L.sleep(2000);
  // 1. permisos revocados (m1 es la «Mac»)
  await m1.ctl({ cmd: "missing", list: ["accessibility"] }); await L.sleep(2500);
  let t = await body(p);
  console.log("PERMISOS:", t.slice(60, 500), await btns(p));
  await L.shot(p, F, "01-permisos");
  L.record(F, "permisos revocados: la UI lo dice y ofrece cómo concederlos", /Faltan permisos/.test(t) && /(Revisar permisos|Abrir ajustes)/i.test(t), t.slice(60, 360));
  await m1.ctl({ cmd: "missing", list: [] }); await L.sleep(2500);
  L.record(F, "permisos concedidos: vuelve el Puesto normal", !/Permisos revocados|permiso/i.test((await body(p)).slice(0, 400)) || /Escritorios en vivo/.test(await body(p)));
  // 2. enlace caído (con la otra todavía «encendida»: pc_lost_after_secs largo en esta máquina)
  const setLost = async (secs) => { m1.kill("SIGTERM"); await L.sleep(1800); const { writeFileSync: w, readFileSync: r } = await import("node:fs"); w(m1.configFile, r(m1.configFile, "utf8").replace(/pc_lost_after_secs = \d+/, `pc_lost_after_secs = ${secs}`)); m1.startAgent(); info("m1", m1); await L.sleep(9000); };
  await setLost(40);
  for (const k of ["usb4", "ethernet", "wifi"]) writeFileSync(`${L.RUN}/${tag}/cut-${k}`, "");
  await L.sleep(9000); t = await body(p);
  console.log("ENLACE:", t.slice(60, 500), await btns(p)); await L.shot(p, F, "02-enlace-caido");
  L.record(F, "enlace caído (la otra sigue encendida): la UI dice «Sin enlace con …» y ofrece volver a probar", /Sin enlace con/.test(t) && /(Probar de nuevo|Reintentar)/.test(t), t.slice(60, 400));
  for (const k of ["usb4", "ethernet", "wifi"]) rmSync(`${L.RUN}/${tag}/cut-${k}`, { force: true });
  await L.sleep(9000);
  L.record(F, "enlace vuelve: el Puesto vuelve solo", /Escritorios en vivo/.test(await body(p)));
  await setLost(2);
  // 3. la otra máquina se pierde (se mata su agente)
  m2.kill("SIGKILL"); await L.sleep(9000); t = await body(p);
  console.log("OTRA PERDIDA:", t.slice(60, 500), await btns(p)); await L.shot(p, F, "03-otra-perdida");
  L.record(F, "otra máquina perdida: estado difícil correcto con su acción", /(apagad|perdi|no responde|sin conexi)/i.test(t) && !/Escritorios en vivo/.test(t), t.slice(60, 400));
  m2.startAgent(); info("m2", m2); await L.sleep(10000);
  L.record(F, "la otra vuelve: el Puesto vuelve solo", /Escritorios en vivo/.test(await body(p)), (await body(p)).slice(60, 200));
  // 4. sin agente
  m1.kill("SIGKILL"); await L.sleep(5000); t = await body(p);
  console.log("SIN AGENTE:", t.slice(60, 500), await btns(p)); await L.shot(p, F, "04-sin-agente");
  L.record(F, "sin agente: la UI explica que tandemd no corre", /tandemd no está corriendo/.test(t), t.slice(60, 400));
  m1.startAgent(); info("m1", m1); await L.sleep(1500);
  await L.sleep(6000);
  L.record(F, "al volver el agente, la UI reconecta sola", /Escritorios en vivo/.test(await body(p)), (await body(p)).slice(60, 200));
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
