import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F4-monitors";
const owner = async (m, name) => (await m.snapshot()).monitors.find((x) => x.name === name)?.owner;
try {
  const p = await L.openUi(m1, { hash: "#/monitors" }); await L.sleep(1500);
  const txt = (await L.text(p));
  L.record(F, "factory monitor shows \"Unsupported\" with its reason", /Factory-32[\s\S]*Unsupported[\s\S]*Firmware without the standard/.test(txt));
  // No writes: the logs are cleared and everything the UI allows on monitors is triggered.
  await m2.ctl({ cmd: "monitor_clear_ops" }); await m1.ctl({ cmd: "monitor_clear_ops" });
  const opts = await p.locator("article", { hasText: "Factory-32" }).locator("[role=radio], button").allInnerTexts();
  console.log("factory card controls:", opts);
  await p.getByRole("button", { name: /What is the standard/ }).click(); await L.sleep(500);
  console.log("\"What is the standard\":", (await L.text(p)).replace(/\n+/g, " | ").slice(-400));
  await L.sleep(6000);
  const ops = [...(await m2.ctl({ cmd: "monitor_ops" })).ops, ...(await m1.ctl({ cmd: "monitor_ops" })).ops];
  const factoryHandles = new Set(); 
  const writes = ops.filter((o) => o.startsWith("Set"));
  L.record(F, "factory monitor: no DDC write on the bus", !ops.some((o) => o.startsWith("Set") && /MonitorHandle\((2)\)/.test(o)), `Set seen: ${writes.length}; total ops ${ops.length}`);
  // Owner re-election when the owner powers off.
  const o1 = await owner(m1, "Std-34"); console.log("current owner:", o1);
  const ownerM = (await m1.snapshot()).agent === o1 ? m1 : m2, other = ownerM === m1 ? m2 : m1;
  ownerM.kill("SIGKILL"); await L.sleep(500);
  const pOther = await L.openUi(other, { hash: "#/monitors" });
  let newOwner = null;
  try { newOwner = await L.until("new owner", async () => { const o = await owner(other, "Std-34"); return o && o !== o1 ? o : null; }, 25000, 500); } catch {}
  await L.sleep(500);
  const txt2 = (await L.text(pOther)).replace(/\n+/g, " | ");
  L.record(F, "the monitor's owner powers off: the other agent becomes the owner", newOwner === (await other.snapshot()).agent, `owner before ${o1}, after ${newOwner}`);
  await L.shot(pOther, F, "owner-reelection");
  console.log(txt2.slice(0, 400));
  // the owner returns
  ownerM.startAgent(); await L.sleep(8000);
  const info = JSON.parse(require("node:fs").readFileSync(`${L.RUN}/${process.argv[2] ?? "host"}/info.json`, "utf8")); info[ownerM === m1 ? "m1" : "m2"].pid = ownerM.pid; require("node:fs").writeFileSync(`${L.RUN}/${process.argv[2] ?? "host"}/info.json`, JSON.stringify(info));
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
