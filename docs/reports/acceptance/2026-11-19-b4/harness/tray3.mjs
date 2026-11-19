import * as L from "./lib.mjs"; import { Mcp } from "./mcp.mjs"; import { mkdirSync } from "node:fs";
const F = "F14-tray-T167";
mkdirSync(L.RUN + "/p16", { recursive: true });
try {
  for (const [tag, banned] of [["win2", /Mac|macOS/], ["mac2", /Windows|\bPC\b/]]) {
    const { m1, m2 } = L.attach(tag);
    const cs = [];
    for (const m of [m1, m2]) { const c = new Mcp(m, L.RUN + "/p16"); await c.init(); const t = await c.call("tunnel_open", { title: "tray-" + m.label }); await c.call("exec", { tunnel: t.value.id, cmd: "sh", args: ["-c", "sleep 90"] }); cs.push(c); }
    await L.sleep(3000);
    for (const m of [m1, m2]) {
      const ctx = await (await L.getBrowser()).newContext({ viewport: { width: 380, height: 600 }, locale: "en-US", colorScheme: "dark" });
      await ctx.addInitScript(L.INIT); const p = await ctx.newPage();
      await p.goto(`${m.origin}/tray.html`); await L.sleep(2500);
      const tx = (await L.text(p)).replace(/\n+/g, " | ");
      console.log(tag, m.label, tx);
      L.record(F, `${tag} ${m.label}: tray singular '1 tunnel', no fixed Mac/PC, no 'tunnels' after 1`, /1 tunnel(?!s)/.test(tx) && !/\b1 tunnels\b/.test(tx) && !banned.test(tx), tx.slice(0, 160));
      await L.shot(p, F, `tray-${tag}-${m.label}`);
      await ctx.close();
    }
    cs.forEach((c) => c.close());
  }
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
