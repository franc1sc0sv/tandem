import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { writeFileSync, rmSync, readFileSync } from "node:fs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F13-trouble-states";
const info = (who, m) => { const i = JSON.parse(readFileSync(`${L.RUN}/${tag}/info.json`, "utf8")); i[who].pid = m.pid; writeFileSync(`${L.RUN}/${tag}/info.json`, JSON.stringify(i)); };
const body = async (p) => (await L.text(p)).replace(/\n+/g, " | ");
const btns = (p) => p.locator("main button").allInnerTexts();
try {
  const p = await L.openUi(m1, { hash: "#/desk" }); await L.sleep(2000);
  // 1. permissions revoked (m1 is the "Mac")
  await m1.ctl({ cmd: "missing", list: ["accessibility"] }); await L.sleep(2500);
  let t = await body(p);
  console.log("PERMISSIONS:", t.slice(60, 500), await btns(p));
  await L.shot(p, F, "01-permissions");
  L.record(F, "permissions revoked: the UI says so and offers how to grant them", /Permissions are missing/.test(t) && /(Review permissions|Open settings)/i.test(t), t.slice(60, 360));
  await m1.ctl({ cmd: "missing", list: [] }); await L.sleep(2500);
  L.record(F, "permissions granted: the Desk returns to normal", !/Permissions revoked|permission/i.test((await body(p)).slice(0, 400)) || /Live desktops/.test(await body(p)));
  // 2. link down (with the other still "on": long pc_lost_after_secs on this machine)
  const setLost = async (secs) => { m1.kill("SIGTERM"); await L.sleep(1800); const { writeFileSync: w, readFileSync: r } = await import("node:fs"); w(m1.configFile, r(m1.configFile, "utf8").replace(/pc_lost_after_secs = \d+/, `pc_lost_after_secs = ${secs}`)); m1.startAgent(); info("m1", m1); await L.sleep(9000); };
  await setLost(40);
  for (const k of ["usb4", "ethernet", "wifi"]) writeFileSync(`${L.RUN}/${tag}/cut-${k}`, "");
  await L.sleep(9000); t = await body(p);
  console.log("LINK:", t.slice(60, 500), await btns(p)); await L.shot(p, F, "02-link-down");
  L.record(F, "link down (the other is still on): the UI says \"No link with …\" and offers to retry", /No link with/.test(t) && /(Test again|Try again|Retry)/.test(t), t.slice(60, 400));
  for (const k of ["usb4", "ethernet", "wifi"]) rmSync(`${L.RUN}/${tag}/cut-${k}`, { force: true });
  await L.sleep(9000);
  L.record(F, "link returns: the Desk recovers by itself", /Live desktops/.test(await body(p)));
  await setLost(2);
  // 3. the other machine is lost (its agent is killed)
  m2.kill("SIGKILL"); await L.sleep(9000); t = await body(p);
  console.log("OTHER LOST:", t.slice(60, 500), await btns(p)); await L.shot(p, F, "03-other-lost");
  L.record(F, "other machine lost: correct trouble state with its action", /(off|lost|not responding|no connection)/i.test(t) && !/Live desktops/.test(t), t.slice(60, 400));
  m2.startAgent(); info("m2", m2); await L.sleep(10000);
  L.record(F, "the other returns: the Desk recovers by itself", /Live desktops/.test(await body(p)), (await body(p)).slice(60, 200));
  // 4. no agent
  m1.kill("SIGKILL"); await L.sleep(5000); t = await body(p);
  console.log("NO AGENT:", t.slice(60, 500), await btns(p)); await L.shot(p, F, "04-no-agent");
  L.record(F, "no agent: the UI explains that tandemd is not running", /tandemd is not running/.test(t), t.slice(60, 400));
  m1.startAgent(); info("m1", m1); await L.sleep(1500);
  await L.sleep(6000);
  L.record(F, "when the agent returns, the UI reconnects by itself", /Live desktops/.test(await body(p)), (await body(p)).slice(60, 200));
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
