// F13: what update_get and the ui-update events say while "Update all" runs (bridge with a test latest.json).
import * as L from "./lib.mjs"; import http from "node:http";
const { m1, m2 } = L.attach(process.argv[2] ?? "r2");
const srv = http.createServer((q, r) => { const b = JSON.stringify({ version: "0.2.0" }); r.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(b), connection: "close" }); r.end(b); }).listen(0, "127.0.0.1");
await new Promise((r) => srv.on("listening", r)); const port = srv.address().port;
try { process.kill(m1.bridgePid, "SIGKILL"); } catch {} await L.sleep(500); await m1.startBridge(["--latest-json", `http://127.0.0.1:${port}/latest.json`]);
const ev = []; const t0 = Date.now();
const ctl = new AbortController();
(async () => { const r = await fetch(`${m1.origin}/__bridge/events`, { signal: ctl.signal }); const rd = r.body.getReader(); const dec = new TextDecoder(); let buf = ""; for (;;) { const { value, done } = await rd.read(); if (done) break; buf += dec.decode(value); let i; while ((i = buf.indexOf("\n\n")) >= 0) { const chunk = buf.slice(0, i); buf = buf.slice(i + 2); const d = chunk.replace(/^data: /, ""); try { const j = JSON.parse(d); if (j.event === "ui-update") ev.push([Date.now() - t0, JSON.stringify(j.payload)]); } catch {} } } })().catch(() => {});
await L.sleep(500);
console.log("check:", JSON.stringify(await m1.invoke("update_check")));
await L.sleep(1500);
console.log("all:", JSON.stringify(await m1.invoke("update_all").catch((e) => e.message)));
for (let i = 0; i < 8; i++) { console.log(Date.now() - t0, "get:", JSON.stringify(await m1.invoke("update_get"))); await L.sleep(1000); }
console.log("events:", ev.map((e) => e.join(" ")).join("\n"));
ctl.abort(); srv.close(); L.saveInfo(process.argv[2] ?? "r2", m1, m2);
