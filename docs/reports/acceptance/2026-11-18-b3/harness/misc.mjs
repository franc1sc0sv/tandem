import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { readFileSync, existsSync } from "node:fs";
const { m1 } = L.attach(process.argv[2] ?? "host"); const D = "http://127.0.0.1:38123";
// (a) window shortcut
{ const F = "F12-settings-palette"; const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1000); await p.getByRole("tab", { name: "Shortcuts" }).click(); await L.sleep(400);
  await p.getByRole("button", { name: /Show or hide Tandem/ }).click(); await L.sleep(200); await p.keyboard.press("Control+Alt+8"); await L.sleep(1200);
  const f = `${m1.cfg}/Tandem/ui.toml`; const txt = existsSync(f) ? readFileSync(f, "utf8") : "(no ui.toml)";
  L.record(F, "Shortcuts: changing \"Show or hide Tandem\" is saved in ui.toml", /Alt\+Digit8|Ctrl\+Alt\+Digit8/.test(txt), txt.replace(/\n/g, " "));
  // the window's local shortcut handles it when the system does not allow registering it
  await p.ctx.close(); }
// (d) crossing test in demo: the 4 states
{ const F = "P6-crossing-test-demo"; for (const [q, re, name] of [["?demo&cross=ok", /(Crossed|OK|passed)/i, "crossed ok"], ["?demo&cross=wrong", /(wrong|edge)/i, "wrong edge"], ["?demo&cross=timeout", /(time|timed|did not cross)/i, "timed out"], ["?demo", /(Waiting|Move the mouse|cross)/i, "waiting"]]) {
  const p = await L.openDemo(D, q, "#/arrangement/crossing"); await L.sleep(q.includes("timeout") ? 14000 : 2500); const t = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, `demo: "${name}" state of the crossing test`, re.test(t) && !/undefined|NaN/.test(t) && p.errors.length === 0, t.slice(60, 260)); await p.ctx.close(); } }
// (e) motion reduced
{ const F = "G8-reduced-motion"; const bad = [];
  for (const [q, h] of [["?demo", "#/desk"], ["?demo", "#/monitors"], ["?demo", "#/tunnels"], ["?demo", "#/network"], ["?demo&wizard=1", ""], ["?demo&txn=run", "#/progress"]]) {
    const p = await L.openDemo(D, q, h, { width: 1280, height: 860 }, "reduce"); await L.sleep(1500);
    const anims = await p.evaluate(() => [...document.querySelectorAll("*")].filter((e) => { const c = getComputedStyle(e); return (c.animationName !== "none" && parseFloat(c.animationDuration) > 0.01) || (c.transitionProperty !== "all" && c.transitionProperty !== "none" && parseFloat(c.transitionDuration) > 0.01) || (c.transitionProperty === "all" && parseFloat(c.transitionDuration) > 0.01); }).slice(0, 4).map((e) => e.tagName + "." + String(e.className).slice(0, 40) + ":" + getComputedStyle(e).animationName + "/" + getComputedStyle(e).transitionDuration));
    if (anims.length) bad.push(`${q}${h}: ${anims.join(", ")}`); await p.ctx.close(); }
  L.record(F, "with prefers-reduced-motion: reduce there are no animations or transitions", bad.length === 0, bad.join(" ; ").slice(0, 500)); }
// (g7) with normal motion there is animation
{ const F = "G7-animations"; const p = await L.openDemo(D, "?demo", "#/desk"); await L.sleep(400);
  const n = await p.evaluate(() => [...document.querySelectorAll("*")].filter((e) => { const c = getComputedStyle(e); return c.animationName !== "none" || parseFloat(c.transitionDuration) > 0; }).length);
  L.record(F, "with normal motion there are animated entrances/transitions on the Desk", n > 5, `${n} elements animated`); await p.ctx.close(); }
await L.closeBrowser();
