import * as L from "./lib.mjs"; import http from "node:http";
import { readFileSync } from "node:fs";
const { m1, m2 } = L.attach("host");
let version = "0.2.0", status = 200;
const srv = http.createServer((req, res) => { if (status !== 200) { res.writeHead(status); return res.end("x"); } const b = JSON.stringify({ version }); res.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(b), connection: "close" }); res.end(b); }).listen(0, "127.0.0.1");
await new Promise((r) => srv.on("listening", r)); const port = srv.address().port;
async function restartBridge(m) { try { process.kill(m.bridgePid, "SIGKILL"); } catch {} await L.sleep(500); await m.startBridge(["--latest-json", `http://127.0.0.1:${port}/latest.json`]); }
const pane = async (p) => (await p.locator("#pane").innerText()).replace(/\n+/g, " | ");
try {
  await restartBridge(m1); await restartBridge(m2);
  const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1200); await p.getByRole("tab", { name: "Updates" }).click(); await L.sleep(800);
  console.log("0:", await pane(p));
  await p.getByRole("button", { name: /Check now/ }).click(); await L.sleep(1500);
  console.log("1 check:", await pane(p));
  await L.shot(p, "F13-updates", "01-new-version");
  await p.getByRole("button", { name: "Update all" }).click();
  for (const t of [300, 1500, 4000, 8000, 12000, 20000]) { await L.sleep(t < 1000 ? 300 : 1500); console.log(`+${t}:`, await pane(p)); }
  await L.shot(p, "F13-updates", "02-updating");
  const tail = readFileSync(m2.log, "utf8").split("\n").filter((l) => /update|Update/i.test(l)).slice(-5).map((l) => l.replace(/\x1b\[[0-9;]*m/g, "").slice(0, 220));
  console.log("agent B:", tail);
  const tail1 = readFileSync(m1.log, "utf8").split("\n").filter((l) => /update|Update/i.test(l)).slice(-5).map((l) => l.replace(/\x1b\[[0-9;]*m/g, "").slice(0, 220));
  console.log("agent A:", tail1);
  status = 404; await p.getByRole("button", { name: /Check/ }).first().click(); await L.sleep(2000);
  console.log("404:", await pane(p), "| toasts:", await p.locator("[aria-label=Notices]").innerText());
  await L.shot(p, "F13-updates", "03-check-404");
  console.log(p.errors);
} catch (e) { console.log("ERROR", e); } finally { srv.close(); await L.closeBrowser(); }
