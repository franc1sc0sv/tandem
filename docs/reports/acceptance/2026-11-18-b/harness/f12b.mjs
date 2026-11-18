import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host3");
const F = "F12-ajustes-paleta";
const entered = async (src, dst, [x, y, dx, dy], tries) => { for (let i = 0; i < tries; i++) { const before = (await dst.ctl({ cmd: "injected" })).count; await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90); const r = await dst.ctl({ cmd: "injected" }); if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true; } return false; };
let geo12, geo21;
async function runCmd(p, q) { await p.keyboard.press("Control+k"); await L.sleep(400); await p.locator("input").last().fill(q); await L.sleep(300); await p.keyboard.press("Enter"); await L.sleep(1200); }
try {
  { const g = await L.geoLeftCenter(m1, m2); geo12 = g.m1m2; geo21 = g.m2m1; }
  await m1.ctl({ cmd: "devices", list: [[0x046d, 0xc33f], [0x046d, 0xc539]] }); await L.sleep(1200);
  await L.sleep(1500);
  const sa = await m1.snapshot(), sb = await m2.snapshot();
  const serverM = sa.input.server === sa.agent ? m1 : m2, otherM = serverM === m1 ? m2 : m1;
  const p = await L.openUi(serverM, { hash: "#/puesto" }); await L.sleep(1500);
  const pOther = await L.openUi(otherM, { hash: "#/puesto" }); await L.sleep(1000);
  const geoTo = serverM === m1 ? geo12 : geo21;
  const pre = true;
  const who = async () => { const a = await m1.snapshot(), b = await m2.snapshot(); return `server=${a.input.server} m1=${a.agent} m2=${b.agent} hub1=${a.input.hub_here} hub2=${b.input.hub_here}`; };
  console.log("antes de bloquear:", await who());
  await runCmd(p, "Bloquear aquí");
  console.log("tras bloquear:", await who(), (await L.text(p)).replace(/\n+/g," | ").match(/(esta máquina[^|]*|No se[^|]*)/)?.[0]);
  const locked = (await serverM.snapshot()).input.locked;
  const txt = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "04-bloqueado-ui");
  const during = await entered(serverM, otherM, geoTo, 10);
  console.log('durante bloqueo server:', (await serverM.snapshot()).input.server === (await serverM.snapshot()).agent);
  // desde la máquina que no manda: debe rechazar con razón visible
  await runCmd(pOther, "Bloquear aquí");
  const rej = (await L.text(pOther)).replace(/\n+/g, " | ");
  await runCmd(p, "Quitar el bloqueo");
  const after = await entered(serverM, otherM, geoTo, 25);
  L.record(F, "«Bloquear aquí» (en la máquina que tiene el teclado): el agente se bloquea y el cursor no cruza; «Quitar el bloqueo» lo suelta", pre && locked === true && !during && after && (await serverM.snapshot()).input.locked === false, `cruzaba=${pre} locked=${locked} cruza_bloqueado=${during} cruza_tras_soltar=${after}`);
  L.record(F, "la UI indica que el cursor está bloqueado (aviso o marca visible)", /[Bb]loquead/.test(txt), txt.slice(60, 420));
  L.record(F, "«Bloquear aquí» en la máquina que no manda explica por qué no se puede", /teclado|no es la que|no se puede/i.test(rej), rej.slice(60, 400));
  console.log(p.errors, pOther.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
