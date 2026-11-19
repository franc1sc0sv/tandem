// F3: "Apply" on a scene card in a freshly opened window (first transaction of the window) and then
// a second time in the same window. Records whether "Changes in progress" is rendered at all.
import * as L from "./lib.mjs";
const tag = process.argv[2] ?? "ow"; const { m1 } = L.attach(tag);
const F = "F3-scenes";
const scenes = (process.argv[3] ?? "Prog A,Work").split(",");
try {
  const p = await L.openUi(m1, { hash: "#/monitors" }); await L.sleep(2000);
  await p.evaluate(() => { window.__seen = []; const t0 = performance.now(); new MutationObserver(() => { const h = document.querySelector("h1")?.textContent ?? ""; const last = window.__seen.at(-1); if (h !== last?.[1]) window.__seen.push([Math.round(performance.now() - t0), h, location.hash]); }).observe(document.body, { subtree: true, childList: true, characterData: true }); });
  const out = [];
  for (const [i, s] of scenes.entries()) {
    await p.evaluate(() => { window.__seen = []; });
    await p.locator(`article[aria-label='Scene ${s}']`).getByRole("button", { name: "Apply" }).click();
    await L.sleep(3000);
    const seen = await p.evaluate(() => window.__seen);
    const shown = seen.some(([, h]) => /Changes in progress/.test(h));
    out.push({ scene: s, shown, seen });
    console.log(`apply #${i + 1} (${s}): progress rendered=${shown}`, JSON.stringify(seen));
    if (i === 0) await L.shot(p, F, "prog-first-apply-fresh-window");
    await p.goto(m1.origin + "/#/monitors"); await L.sleep(1500);
  }
  L.record(F, "first Apply in a freshly opened window shows Changes in progress", out[0].shown, JSON.stringify(out[0].seen));
  if (out[1]) L.record(F, "second Apply in the same window shows Changes in progress", out[1].shown, JSON.stringify(out[1].seen));
  console.log(p.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
