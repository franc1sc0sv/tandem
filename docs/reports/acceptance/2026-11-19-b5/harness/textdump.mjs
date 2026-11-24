import * as L from "./lib.mjs"; import { writeFileSync } from "node:fs";
const O = "http://127.0.0.1:38123";
const routes = ["#/desk", "#/tunnels", "#/monitors", "#/monitors/scene", "#/arrangement", "#/arrangement/crossing", "#/network", "#/machine/a", "#/machine/b", "#/settings", "#/states", "#/progress"];
const qs = ["?demo", "?demo&trouble=no-agent", "?demo&trouble=peer-off", "?demo&trouble=link-down", "?demo&trouble=permissions", "?demo&txn=run", "?demo&txn=ok", "?demo&txn=fail", "?demo&wizard=1", "?demo&notices=1"];
const seen = new Set(); const out = [];
const grab = async (p, label) => {
  const items = await p.evaluate(() => {
    const r = [];
    for (const e of document.querySelectorAll("*")) {
      for (const a of ["aria-label", "title", "placeholder", "alt"]) { const v = e.getAttribute?.(a); if (v) r.push(a + ": " + v); }
    }
    r.push(...document.body.innerText.split("\n").map((s) => "text: " + s.trim()).filter((s) => s.length > 6));
    return r;
  });
  for (const i of items) if (!seen.has(i)) { seen.add(i); out.push(`[${label}] ${i}`); }
};
try {
  for (const q of qs) for (const r of (q === "?demo" ? routes : ["#/desk"])) {
    const hash = q.includes("txn") ? "#/progress" : r;
    const p = await L.openDemo(O, q, hash); await L.sleep(1000);
    if (hash === "#/settings") for (const tab of ["Desk and machines", "Shortcuts", "Keyboard and mouse", "Updates", "Advanced"]) { await p.getByRole("tab", { name: tab }).click().catch(() => {}); await L.sleep(300); await grab(p, "settings:" + tab); }
    await grab(p, q + r);
    await p.ctx.close();
  }
  // wizard steps in demo
  const p = await L.openDemo(O, "?demo&wizard=1", ""); await L.sleep(1200);
  for (const s of ["Your desk", "Permissions", "Keyboard and mouse", "Arrangement and crossing test", "Done"]) { await p.locator("aside[aria-label=Steps] button", { hasText: s }).click().catch(() => {}); await L.sleep(500); await grab(p, "wizard:" + s); }
  writeFileSync(L.RUN + "/textdump.txt", out.join("\n")); console.log(out.length, "strings");
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
