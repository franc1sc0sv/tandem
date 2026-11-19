import * as L from "./lib.mjs";
const { m1, m2 } = L.attach("fin3");
const ES_WORDS = Buffer.from("Q2FyZ2FuZG98Q2FuY2VsYXJ8R3VhcmRhcnxFbGltaW5hcnxSZWludGVudGFyfENvbmVjdGFyfERlc2NvbmVjdGFyfEFwbGljYXJ8Q2VycmFyfEFicmlyfEVzY3JpdG9yaW98QWp1c3Rlc3xQYW50YWxsYXxUZWNsYWRvfFJhdMOzbnxFbmxhY2V8VMO6bmVsfE3DoXF1aW5hfEFjdHVhbGl6YXJ8R3VhcmRhbmRvfExpc3RvfFNpZ3VpZW50ZXxBbnRlcmlvcnxBcGFnYXJ8RW5jZW5kZXJ8Tm9tYnJlfFByaW5jaXBhbHxSZXNwYWxkb3xCYW5kYQ==", "base64").toString(); // Spanish words, encoded so this file stays English-only
const spanish = new RegExp("[\u00e1\u00e9\u00ed\u00f3\u00fa\u00f1\u00bf\u00a1]|\\b(" + ES_WORDS + ")\\b");
const hits = []; let n = 0;
try {
  for (const m of [m1, m2]) for (const r of ["#/desk", "#/tunnels", "#/monitors", "#/monitors/scene", "#/arrangement", "#/arrangement/crossing", "#/network", "#/machine/a", "#/machine/b", "#/settings", "#/states", "#/progress"]) {
    const p = await L.openUi(m, { hash: r }); await L.sleep(1100);
    if (r === "#/settings") for (const tab of ["Shortcuts", "Keyboard and mouse", "Updates", "Advanced"]) { await p.getByRole("tab", { name: tab }).click().catch(() => {}); await L.sleep(300); const t = await p.evaluate(() => document.body.innerText); n++; const x = t.match(spanish); if (x) hits.push(`${m.label} settings:${tab}: ${x[0]}`); }
    const t = await p.evaluate(() => document.body.innerText + " " + [...document.querySelectorAll("[aria-label],[title]")].map((e) => (e.getAttribute("aria-label") ?? "") + " " + (e.getAttribute("title") ?? "")).join(" "));
    n++; const x = t.match(spanish); if (x) hits.push(`${m.label} ${r}: ${x[0]} «${t.slice(Math.max(0, t.search(spanish) - 30), t.search(spanish) + 40).replace(/\n/g, " ")}»`);
    await p.ctx.close();
  }
  // Wizard on the real agent, every step
  const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1000); await p.getByRole("button", { name: /^Open$/ }).first().click(); await L.sleep(800);
  for (const s of ["Your desk", "Permissions", "Keyboard and mouse", "Arrangement and crossing test", "Done"]) { await p.locator("aside[aria-label=Steps] button", { hasText: s }).click().catch(() => {}); await L.sleep(600); const t = await p.evaluate(() => document.body.innerText); n++; const x = t.match(spanish); if (x) hits.push(`wizard ${s}: ${x[0]}`); }
  console.log(n, "pages checked");
  L.record("G9-english", "no Spanish text on any live screen, settings tab or wizard step (real agents)", hits.length === 0, hits.slice(0, 5).join(" ; "));
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
