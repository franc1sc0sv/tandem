import * as L from "./lib.mjs"; import { Mcp } from "./mcp.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "win2"); const F = "P10-notices";
try {
  const dir = L.RUN + "/p16";
  const c = new Mcp(m1, dir); await c.init(); const t = await c.call("tunnel_open", { title: "notices2" });
  const p = await L.openUi(m1, { hash: "#/desk" }); await L.sleep(1500);
  await p.evaluate(() => { window.__seen = new Set(); const grab = () => { const r = document.querySelector("[aria-label=Notices]"); if (r) for (const n of r.querySelectorAll("*")) { const t = n.innerText?.replace(/\s+/g, " ").trim(); if (t && t.length < 200 && /big|MB|%|Copied|Sending/.test(t)) window.__seen.add(t); } }; new MutationObserver(grab).observe(document.body, { subtree: true, childList: true, characterData: true }); setInterval(grab, 50); });
  const r = await c.call("files_put", { tunnel: t.value.id, local: dir + "/big.bin", remote: `${m2.dataDir}/workspaces/${t.value.id}/big.bin` });
  await L.sleep(1000);
  const seen = await p.evaluate(() => [...window.__seen]);
  console.log("put ok:", r.ok, JSON.stringify(seen).slice(0, 900));
  L.record(F, "a file copy from a tunnel shows a notice with progress and a final 'Copied' (DOM observer)", r.ok && seen.some((s) => /Sending/.test(s) && /%/.test(s)) && seen.some((s) => /Copied/.test(s)), JSON.stringify(seen).slice(0, 300));
  await c.call("tunnel_close", { tunnel: t.value.id }); c.close();
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); process.exit(0); }
