import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { writeFileSync, rmSync } from "node:fs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F7 = "F7-no-owner", F8 = "F8-clipboard";
const cut = (on) => { for (const k of ["usb4", "ethernet", "wifi"]) { if (on) writeFileSync(`${L.RUN}/${tag}/cut-${k}`, ""); else rmSync(`${L.RUN}/${tag}/cut-${k}`, { force: true }); } };
let geo12, geo21;
const entered = async (src, dst, [x, y, dx, dy], tries) => { for (let i = 0; i < tries; i++) { const before = (await dst.ctl({ cmd: "injected" })).count; await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90); const r = await dst.ctl({ cmd: "injected" }); if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true; } return false; };
const srv = async () => (await m1.snapshot()).input.server;
const sideOf = async (m) => (await m.snapshot()).agent;
const tail = async (m, n = 12) => (await m.ctl({ cmd: "injected" })).tail.slice(0, n);
try {
  { const g = await L.geoLeftCenter(m1, m2); geo12 = g.m1m2; geo21 = g.m2m1; }
  await m1.ctl({ cmd: "devices", list: [[0x046d, 0xc33f], [0x046d, 0xc539]] }); await L.sleep(1500);
  const a1 = await sideOf(m1), a2 = await sideOf(m2);
  // (a) the one in use takes control
  const use = async (m, ms = 900) => { const t = Date.now(); while (Date.now() - t < ms) { await m.ctl({ cmd: "rel", dx: 0, dy: 1 }); await L.sleep(40); } await L.sleep(500); };
  await use(m1); const s1 = await srv(); await use(m2); const s2 = await srv(); await use(m1); const s3 = await srv();
  L.record(F7, "physical input on A and then on B: the one in use takes control", s1 === a1 && s2 === a2 && s3 === a1, `server after use m1=${s1}, m2=${s2}, m1=${s3} (m1=${a1}, m2=${a2})`);
  // (b) no stuck keys when the owner changes with a key held on the remote
  await use(m1); await entered(m1, m2, geo12, 25);
  await m1.ctl({ cmd: "key", usage: 0x04, down: true }); await L.sleep(400);
  const injA = (await tail(m2, 30)).filter((e) => e.includes("Key"));
  await use(m2, 900); // the user takes the other one's mouse
  const tl = await tail(m2, 40);
  const pressed = tl.some((e) => /usage.*0x?4|Pressed/.test(e) && e.includes("Pressed")); const released = tl.some((e) => e.includes("Released"));
  L.record(F7, "when the owner changes with a key held, it does not stay stuck on the other machine", injA.some((e) => e.includes("Pressed")) && released, `injected before: ${JSON.stringify(injA.slice(0, 2))}; after: ${JSON.stringify(tl.filter((e) => e.includes("Key")).slice(0, 3))}`);
  await m1.ctl({ cmd: "key", usage: 0x04, down: false });
  // (c) no stuck keys when the link drops
  await use(m1); await entered(m1, m2, geo12, 25);
  await m1.ctl({ cmd: "key", usage: 0x05, down: true }); await L.sleep(500);
  const n0 = (await m2.ctl({ cmd: "injected" })).count; cut(true); await L.sleep(5000);
  const tl2 = (await tail(m2, 40)); const rel2 = tl2.some((e) => e.includes("Released") && !tl2.slice(0, 0).length);
  const newEv = (await m2.ctl({ cmd: "injected" })).count - n0;
  L.record(F7, "when the link drops with a key held, the other machine releases it", tl2.slice(0, Math.max(newEv, 1)).some((e) => e.includes("Released")), `new events after the cut: ${JSON.stringify(tl2.slice(0, Math.min(newEv, 4)))}`);
  cut(false); await m1.ctl({ cmd: "key", usage: 0x05, down: false }); await L.sleep(9000);
  // F8: clipboard text e image A→B y B→A
  for (const [from, to, nm] of [[m1, m2, "A→B"], [m2, m1, "B→A"]]) {
    await from.ctl({ cmd: "clip_copy", text: `text-${nm}` }); await L.sleep(1800);
    const got = await to.ctl({ cmd: "clip_get" });
    L.record(F8, `copy text on ${nm} and paste it on the other`, got.text === `text-${nm}`, JSON.stringify(got));
  }
  await m1.ctl({ cmd: "clip_copy", image: [4, 3] }); await L.sleep(2000); const gi = await m2.ctl({ cmd: "clip_get" });
  L.record(F8, "copy an image on A and paste it on B", JSON.stringify(gi.image) === "[4,3]", JSON.stringify(gi));
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); cut(false); await L.closeBrowser(); }
