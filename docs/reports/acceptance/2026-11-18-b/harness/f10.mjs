import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { readFileSync, existsSync } from "node:fs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F10-power";
const steps = (m) => existsSync(m.dataDir + "/sim-steps.jsonl") ? readFileSync(m.dataDir + "/sim-steps.jsonl", "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.stringify(JSON.parse(l))) : [];
async function runCmd(p, q) { await p.keyboard.press("Control+k"); await L.sleep(400); await p.locator("input").last().fill(q); await L.sleep(300); await p.keyboard.press("Enter"); await L.sleep(500); }
try {
  const p = await L.openUi(m1, { hash: "#/desk" }); await L.sleep(1500);
  const s0 = [steps(m1).length, steps(m2).length];
  await runCmd(p, "Apagar");
  const seen = []; for (let i = 0; i < 16; i++) { const h = await p.evaluate(() => location.hash); const t = (await L.text(p)).replace(/\n+/g, " | "); seen.push(h + " :: " + (t.match(/(Powering off|is off|powered off|In progress|Changes in progress)[^|]*/)?.[0] ?? "")); await L.sleep(500); }
  const u = [...new Set(seen)]; console.log("power off, states seen:", u);
  await L.shot(p, F, "01-power-off");
  const st1 = (await m1.snapshot()).state, s1 = [steps(m1), steps(m2)];
  L.record(F, "power off the other machine: transaction with steps in the agent (PowerOff) and state off", s1[0].length + s1[1].length > s0[0] + s0[1] && st1.on.a !== st1.on.b, `steps: ${JSON.stringify(s1)} state ${JSON.stringify(st1)}`);
  await runCmd(p, "Encender");
  const seen2 = []; for (let i = 0; i < 24; i++) { const t = (await L.text(p)).replace(/\n+/g, " | "); seen2.push(t.match(/(Powering on|Starting|In progress|PC is off|is on|Wake|waking)[^|]*/)?.[0] ?? ""); await L.sleep(500); }
  console.log("power on, states seen:", [...new Set(seen2)]);
  const st2 = (await m1.snapshot()).state, s2 = [steps(m1), steps(m2)];
  await L.shot(p, F, "02-power-on");
  L.record(F, "power on the other machine (simulated Wake-on-LAN over Ethernet): PowerOn step and state on", s2[0].length + s2[1].length > s1[0].length + s1[1].length && st2.on.a && st2.on.b, `steps: ${JSON.stringify(s2)} state ${JSON.stringify(st2)}`);
  console.log(p.errors); await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
