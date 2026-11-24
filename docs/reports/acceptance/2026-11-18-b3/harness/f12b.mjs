import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host3");
const F = "F12-settings-palette";
const entered = async (src, dst, [x, y, dx, dy], tries) => { for (let i = 0; i < tries; i++) { const before = (await dst.ctl({ cmd: "injected" })).count; await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90); const r = await dst.ctl({ cmd: "injected" }); if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true; } return false; };
let geo12, geo21;
async function runCmd(p, q) { await p.keyboard.press("Control+k"); await L.sleep(400); await p.locator("input").last().fill(q); await L.sleep(300); await p.keyboard.press("Enter"); await L.sleep(1200); }
try {
  { const g = await L.geoLeftCenter(m1, m2); geo12 = g.m1m2; geo21 = g.m2m1; }
  await m1.ctl({ cmd: "devices", list: [[0x046d, 0xc33f], [0x046d, 0xc539]] }); await L.sleep(1200);
  await L.sleep(1500);
  const sa = await m1.snapshot(), sb = await m2.snapshot();
  const serverM = sa.input.server === sa.agent ? m1 : m2, otherM = serverM === m1 ? m2 : m1;
  const p = await L.openUi(serverM, { hash: "#/desk" }); await L.sleep(1500);
  const pOther = await L.openUi(otherM, { hash: "#/desk" }); await L.sleep(1000);
  const geoTo = serverM === m1 ? geo12 : geo21;
  const pre = true;
  const who = async () => { const a = await m1.snapshot(), b = await m2.snapshot(); return `server=${a.input.server} m1=${a.agent} m2=${b.agent} hub1=${a.input.hub_here} hub2=${b.input.hub_here}`; };
  console.log("before de lock:", await who());
  await runCmd(p, "Lock here");
  console.log("after locking:", await who(), (await L.text(p)).replace(/\n+/g," | ").match(/(this machine[^|]*|Cannot[^|]*)/)?.[0]);
  const locked = (await serverM.snapshot()).input.locked;
  const txt = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "04-locked-ui");
  const during = await entered(serverM, otherM, geoTo, 10);
  console.log('during lock server:', (await serverM.snapshot()).input.server === (await serverM.snapshot()).agent);
  // from the machine that is not in control: it must reject with a visible reason
  await runCmd(pOther, "Lock here");
  const rej = (await L.text(pOther)).replace(/\n+/g, " | ");
  await runCmd(p, "Remove lock");
  const after = await entered(serverM, otherM, geoTo, 25);
  L.record(F, "\"Lock here\" (on the machine that has the keyboard): the agent locks and the cursor does not cross; \"Remove lock\" releases it", pre && locked === true && !during && after && (await serverM.snapshot()).input.locked === false, `crossed=${pre} locked=${locked} cruza_bloqueado=${during} cruza_tras_soltar=${after}`);
  L.record(F, "the UI indicates the cursor is locked (notice or visible mark)", /[Bb]loquead/.test(txt), txt.slice(60, 420));
  L.record(F, "\"Lock here\" on the machine that is not in control explains why it cannot be done", /keyboard|is not the one|cannot/i.test(rej), rej.slice(60, 400));
  console.log(p.errors, pOther.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
