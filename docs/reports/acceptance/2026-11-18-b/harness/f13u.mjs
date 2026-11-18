import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import http from "node:http"; import { spawn } from "node:child_process"; import { openSync, readFileSync, existsSync } from "node:fs"; import { join } from "node:path";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F13-actualizaciones";
let version = "0.2.0", status = 200;
const srv = http.createServer((req, res) => { if (status !== 200) { res.writeHead(status); return res.end("x"); } const b = JSON.stringify({ version }); res.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(b), connection: "close" }); res.end(b); }).listen(0, "127.0.0.1");
await new Promise((r) => srv.on("listening", r)); const port = srv.address().port;
async function restartBridge(m) { try { process.kill(m.bridgePid, "SIGKILL"); } catch {} await L.sleep(500); await m.startBridge(["--latest-json", `http://127.0.0.1:${port}/latest.json`]); }
try {
  await restartBridge(m1); await restartBridge(m2);
  const p = await L.openUi(m1, { hash: "#/ajustes" }); await L.sleep(1200); await p.getByRole("tab", { name: "Actualizaciones" }).click(); await L.sleep(800);
  const t0 = (await p.locator("#pane").innerText()).replace(/\n+/g, " | ");
  L.record(F, "muestra las versiones de las dos máquinas", /0\.1\.0/.test(t0), t0.slice(0, 220));
  await p.getByRole("button", { name: /Buscar ahora/ }).click(); await L.sleep(1500);
  const t1 = (await p.locator("#pane").innerText()).replace(/\n+/g, " | ");
  await L.shot(p, F, "01-version-nueva");
  L.record(F, "buscar contra un latest.json de prueba encuentra la versión nueva", /0\.2\.0/.test(t1) && (await p.getByRole("button", { name: "Actualizar todas" }).count()) === 1, t1.slice(0, 300));
  await p.getByRole("button", { name: "Actualizar todas" }).click(); await L.sleep(2500);
  const t2 = (await p.locator("#pane").innerText()).replace(/\n+/g, " | ");
  const logs = readFileSync(m2.log, "utf8").split("\n").filter((l) => /UpdatePeer|install/i.test(l)).slice(-2).map((l) => l.replace(/\x1b\[[0-9;]*m/g, "").slice(0, 200));
  const st2 = await m2.snapshot(); 
  const pend = await (async () => { const r = await fetch(`${m2.origin}/__bridge/invoke/update_get`, { method: "POST", body: "{}" }); return r.json(); })();
  await L.shot(p, F, "02-actualizando");
  L.record(F, "«Actualizar todas»: primero le pide a la otra máquina por tandemd (plan de las dos)", /otra|Pidiendo|Esperando|actualiz/i.test(t2) && (logs.length > 0 || pend.step !== undefined), `UI: ${t2.slice(0, 220)} | agente B: ${JSON.stringify(logs)} | B.update=${JSON.stringify(pend)}`);
  await L.sleep(22000);
  const t3 = (await p.locator("#pane").innerText()).replace(/\n+/g, " | ");
  L.record(F, "si la otra no vuelve con la versión nueva, esta no se actualiza y la UI lo explica (vuelta atrás)", /no volvió|se queda|otra máquina/i.test(t3), t3.slice(0, 300));
  // 404
  status = 404; await p.getByRole("button", { name: /Buscar|Comprobar/ }).first().click(); await L.sleep(1800);
  const t4 = (await p.locator("#pane").innerText()).replace(/\n+/g, " | ");
  L.record(F, "un latest.json inexistente muestra el error al buscar", /no se pudo buscar|404|Error/i.test(t4), t4.slice(0, 260));
  // Actualizar solo
  await p.getByRole("switch", { name: "Actualizar solo" }).click(); await L.sleep(800);
  const f = join(m1.cfg, "Tandem", "update.toml");
  L.record(F, "«Actualizar solo» se guarda en update.toml", existsSync(f) && /auto\s*=\s*(true|false)/.test(readFileSync(f, "utf8")), existsSync(f) ? readFileSync(f, "utf8").replace(/\n/g, " ") : "(sin archivo)");
  console.log(p.errors); srv.close(); await L.closeBrowser();
} catch (e) { console.log("ERROR", e); srv.close(); await L.closeBrowser(); }
