import * as L from "./lib.mjs"; import { Mcp } from "./mcp.mjs"; import { writeFileSync, mkdirSync } from "node:fs"; import { execFileSync } from "node:child_process";
const { m1, m2 } = L.attach(process.argv[2] ?? "fin"); const F = "P10-avisos";
try {
  mkdirSync("/tmp/claude-1000/accB2run/p16", { recursive: true }); execFileSync("dd", ["if=/dev/urandom", "of=/tmp/claude-1000/accB2run/p16/grande.bin", "bs=1M", "count=500"], { stdio: "ignore" });
  const c = new Mcp(m1, "/tmp/claude-1000/accB2run/p16"); await c.init(); const t = await c.call("tunnel_open", { title: "avisos" });
  const p = await L.openUi(m1, { hash: "#/puesto" }); const p2 = await L.openUi(m2, { hash: "#/puesto" }); await L.sleep(1500);
  const seen = new Set(); let shot = false;
  const put = c.call("files_put", { tunnel: t.value.id, local: "/tmp/claude-1000/accB2run/p16/grande.bin", remote: `${m2.dataDir}/workspaces/${t.value.id}/grande.bin` });
  for (let i = 0; i < 120; i++) { const tx = (await p.locator("[aria-label=Avisos]").innerText().catch(() => "")).replace(/\n+/g, " | "); if (tx) { seen.add(tx.slice(0, 160)); if (!shot) { await L.shot(p, F, "01-aviso-copia"); shot = true; } } await L.sleep(150); }
  const r = await put; console.log("put:", JSON.stringify(r).slice(0,400)); console.log("seen:", JSON.stringify([...seen]));
  L.record(F, "una copia de archivos de un túnel muestra un aviso con progreso en la UI", [...seen].some((s) => /grande|Subiendo|Copiando|MB|%|archivo/i.test(s)) && r.ok, `avisos vistos: ${JSON.stringify([...seen]).slice(0, 300)}`);
  await c.call("tunnel_close", { tunnel: t.value.id }); c.close(); await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
