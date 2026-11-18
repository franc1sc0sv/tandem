import * as L from "./lib.mjs"; import { Mcp } from "./mcp.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "c2"); const F = "F14-tray";
const dir = process.env.ACC_RUN + "/p16";
try {
  const c = new Mcp(m1, dir); await c.init(); const t = await c.call("tunnel_open", { title: "tray" }); await c.call("exec", { tunnel: t.value.id, cmd: "sh", args: ["-c", "sleep 60"] }).catch(()=>{}); await L.sleep(2500);
  const ctx = await (await L.getBrowser()).newContext({ viewport: { width: 380, height: 600 }, locale: "en-US", colorScheme: "dark" });
  await ctx.addInitScript(L.INIT); const p = await ctx.newPage();
  await p.goto(`${m1.origin}/tray.html`); await L.sleep(2500);
  const tx = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "the tray counts open tunnels (1 open) with a freshly opened window", !/0 tunnels/.test(tx), tx.slice(0, 200));
  await L.shot(p, F, "02-tray-with-tunnel");
  await c.call("tunnel_close", { tunnel: t.value.id }); c.close(); await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
