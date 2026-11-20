import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { readFileSync, existsSync } from "node:fs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F12-settings-palette";
const steps = (m) => existsSync(m.dataDir + "/sim-steps.jsonl") ? readFileSync(m.dataDir + "/sim-steps.jsonl", "utf8").trim().split("\n").filter(Boolean).length : 0;
const entered = async (src, dst, [x, y, dx, dy], tries) => { for (let i = 0; i < tries; i++) { const before = (await dst.ctl({ cmd: "injected" })).count; await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90); const r = await dst.ctl({ cmd: "injected" }); if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true; } return false; };
async function palette(p, query) { await p.keyboard.press("Control+k"); await L.sleep(400); const inp = p.locator("[role=dialog] input, [role=combobox], input[type=text], input").last(); await inp.fill(query); await L.sleep(300); }
async function runCmd(p, query) { await palette(p, query); const opts = await p.locator("[role=option]").allInnerTexts(); await p.keyboard.press("Enter"); await L.sleep(800); return opts; }
try {
  const p = await L.openUi(m1, { hash: "#/desk" }); await L.sleep(1800);
  await p.keyboard.press("Control+k"); await L.sleep(500);
  const all = (await p.locator("[role=option]").allInnerTexts()).map((x) => x.replace(/\n/g, " ")); console.log("COMANDOS:", all);
  await L.shot(p, F, "01-palette"); await p.keyboard.press("Escape"); await L.sleep(300);
  const wantTitles = ["Apply", "New scene", "Power off", "Change the other machine", "Monitors", "Network", "Arrangement", "Settings", "Desk", "Tunnels", "Measure the link", all.some((a) => a.includes("Remove the lock")) ? "Remove the lock" : "Lock here", "Open setup"];
  L.record(F, "the palette lists all D15 commands", wantTitles.every((w) => all.some((a) => a.includes(w))), "missing: " + wantTitles.filter((w) => !all.some((a) => a.includes(w))).join(","));
  // navigation
  for (const [q, hash] of [["Monitors", "#/monitors"], ["Network", "#/network"], ["Arrangement", "#/arrangement"], ["Settings", "#/settings"], ["Tunnels", "#/tunnels"], ["Desk", "#/desk"]]) {
    await runCmd(p, q); const h = await p.evaluate(() => location.hash); L.record(F, `palette «${q}» navigates a ${hash}`, h === hash, h);
  }
  await p.keyboard.press("Control+,"); await L.sleep(500); L.record(F, "Ctrl+, opens Settings", (await p.evaluate(() => location.hash)) === "#/settings");
  // setup
  await runCmd(p, "Open setup"); L.record(F, "palette \"Open setup\" opens it", (await p.locator('[role=dialog][aria-label=Setup]').count()) === 1); await p.keyboard.press("Escape"); await L.sleep(500);
  await runCmd(p, "Change the other machine"); const w1 = await p.locator('[role=dialog][aria-label=Setup] [aria-current=step]').innerText().catch(() => ""); L.record(F, "palette \"Change the other machine…\" opens setup at \"Your desk\"", /Your desk/.test(w1), w1); await p.keyboard.press("Escape"); await L.sleep(500);
  // measure
  await runCmd(p, "Measure the link"); await L.sleep(7000);
  const net = (await p.evaluate(() => location.hash)); const txt = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "palette \"Measure the link\" asks the agent for the measurement and the UI gives the value or the reason", /Mbps|Gbps|Measuring|bandwidth|Not measured/i.test(txt), `${net} ${txt.slice(60, 300)}`);
  await p.goto(m1.origin + "/#/desk"); await L.sleep(1500);
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
