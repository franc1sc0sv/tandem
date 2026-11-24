// F13 "Update all": while the bridge says step = wait_peer, what the Updates pane shows (T-251).
import * as L from "./lib.mjs"; import http from "node:http";
const tag = process.argv[2] ?? "r2"; const { m1, m2 } = L.attach(tag); const F = "F13-updates";
const srv = http.createServer((q, r) => { const b = JSON.stringify({ version: "0.2.0" }); r.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(b), connection: "close" }); r.end(b); }).listen(0, "127.0.0.1");
await new Promise((r) => srv.on("listening", r)); const port = srv.address().port;
try { process.kill(m1.bridgePid, "SIGKILL"); } catch {} await L.sleep(500); await m1.startBridge(["--latest-json", `http://127.0.0.1:${port}/latest.json`]); L.saveInfo(tag, m1, m2);
try {
  const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1200); await p.getByRole("tab", { name: "Updates" }).click(); await L.sleep(800);
  await p.getByRole("button", { name: /Check now/ }).click(); await L.sleep(1500);
  await p.getByRole("button", { name: "Update all" }).click(); await L.sleep(2000);
  const st = await m1.invoke("update_get");
  const pane = (await p.locator("#pane").innerText()).replace(/\n+/g, " | ");
  const enabled = await p.getByRole("button", { name: "Update all" }).isEnabled();
  await L.shot(p, F, "T-251-updating-pane-stale");
  let again = ""; if (enabled) { await p.getByRole("button", { name: "Update all" }).click(); await L.sleep(800); again = (await p.locator("[role=status], [role=alert]").allInnerTexts().catch(() => [])).join(" / "); }
  console.log("bridge step:", st.step, "| pane:", pane.slice(0, 300), "| Update all enabled:", enabled, "| second click:", again);
  L.record(F, "while the update runs (step wait_peer) the pane shows the step and \"Update all\" is disabled", st.step === "wait_peer" && /The other machine is updating|Asking the other machine/.test(pane) && !enabled, `step=${st.step}; enabled=${enabled}; second click: ${again}; pane: ${pane.slice(0, 200)}`);
  console.log(p.errors);
} catch (e) { console.log("ERROR", e); } finally { srv.close(); await L.closeBrowser(); process.exit(0); }
