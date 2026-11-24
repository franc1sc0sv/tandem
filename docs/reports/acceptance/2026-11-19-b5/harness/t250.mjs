// T-250: "Save and apply" must always show Changes in progress (F-Transactions), not land on
// Monitors. N runs (default 12), each in a fresh window, alternating which machine Std-34 shows so
// every run is a real transaction; then N "Apply" clicks on scene cards, also in fresh windows.
// For each run: the progress heading was rendered, the double's bus really switched, and the
// transaction ended with success. Usage: node t250.mjs <tag> [N]
import * as L from "./lib.mjs";
const tag = process.argv[2] ?? "fin"; const N = Number(process.argv[3] ?? 12); const { m1, m2 } = L.attach(tag);
const F = "T-250-save-and-apply";
const pickRadio = (p, label, opt) => p.locator(".f", { hasText: `${label} shows` }).locator("[role=radio]", { hasText: opt }).click();
// Each host has its own DDC double: the bus that moves is the owner's (read who owns Std-34 from the agent).
const bus = async () => { const s = await m1.snapshot(); const v = s.monitors.find((m) => m.name === "Std-34"); const owner = v.owner === s.agent ? m1 : m2; return (await owner.ctl({ cmd: "monitor_screen", id: 0 })).input; };
const want = (name) => (name === m1.name ? "Hdmi" : "DisplayPort"); // m1 reaches Std-34 over HDMI, m2 over DP (t3.mjs)
async function watch(p) {
  await p.evaluate(() => { window.__seen = []; const t0 = performance.now(); const rec = () => { const h = document.querySelector("h1")?.textContent ?? ""; const last = window.__seen.at(-1); if (h !== last?.[1] || location.hash !== last?.[2]) window.__seen.push([Math.round(performance.now() - t0), h, location.hash]); }; new MutationObserver(rec).observe(document.body, { subtree: true, childList: true, characterData: true }); addEventListener("hashchange", rec); });
}
// Live names: earlier flows (F9) may have renamed the machines.
{ const s = await m1.snapshot(); m1.name = s.desk.machines[s.agent].name; m2.name = s.desk.machines[s.agent === "a" ? "b" : "a"].name; }
const runs = []; const P = String(Date.now() % 100000); // unique scene names per invocation
try {
  const before = await bus();
  // Start from the opposite of the first target so run 1 is a real change.
  for (let i = 0; i < N; i++) {
    const cur = await bus();
    const target = cur === "Hdmi" ? m2.name : m1.name;
    const p = await L.openUi(m1, { hash: "#/monitors/scene" }); await L.sleep(1500);
    await watch(p);
    await p.locator("input").first().fill(`T250 ${P}-${i + 1}`);
    await pickRadio(p, "Std-34", target);
    await p.getByRole("button", { name: "Save and apply" }).click();
    let progressText = "";
    for (let k = 0; k < 40; k++) { const t = await L.text(p); if (/Changes in progress/.test(t)) progressText = t.replace(/\n+/g, " | "); await L.sleep(100); }
    await L.sleep(1500);
    const seen = await p.evaluate(() => window.__seen);
    const shown = seen.some(([, h]) => /Changes in progress/.test(h));
    const final = (await L.text(p)).replace(/\n+/g, " | ");
    const b = await bus();
    const ok = shown && b === want(target) && p.errors.length === 0;
    runs.push({ kind: "save-and-apply", i: i + 1, target, shown, bus: b, ok, seen: seen.map(([t, h, x]) => `${t}:${h}@${x}`).join(" > "), errors: p.errors });
    console.log(JSON.stringify(runs.at(-1)));
    if (i === 0) await L.shot(p, F, "01-save-and-apply-progress");
    if (!ok) await L.shot(p, F, `fail-save-and-apply-${i + 1}`);
    await p.ctx.close();
  }
  // Apply from a scene card (first transaction of a fresh window each time).
  for (let i = 0; i < N; i++) {
    const cur = await bus();
    // Pick a T250 scene whose target differs from the current input.
    const names = [...new Set(runs.filter((r) => r.kind === "save-and-apply" && want(r.target) !== cur).map((r) => `T250 ${P}-${r.i}`))];
    const name = names[i % names.length];
    const p = await L.openUi(m1, { hash: "#/monitors" }); await L.sleep(1500);
    await watch(p);
    await p.locator(`article[aria-label='Scene ${name}']`).getByRole("button", { name: "Apply" }).click();
    await L.sleep(4000);
    const seen = await p.evaluate(() => window.__seen);
    const shown = seen.some(([, h]) => /Changes in progress/.test(h));
    const b = await bus();
    const ok = shown && b !== cur && p.errors.length === 0;
    runs.push({ kind: "apply-card", i: i + 1, scene: name, shown, from: cur, bus: b, ok, seen: seen.map(([t, h, x]) => `${t}:${h}@${x}`).join(" > "), errors: p.errors });
    console.log(JSON.stringify(runs.at(-1)));
    if (!ok) await L.shot(p, F, `fail-apply-card-${i + 1}`);
    await p.ctx.close();
  }
  const sa = runs.filter((r) => r.kind === "save-and-apply"), ac = runs.filter((r) => r.kind === "apply-card");
  L.record(F, `Save and apply shows Changes in progress and switches the monitor in every run (${sa.filter((r) => r.ok).length}/${sa.length})`, sa.length >= 10 && sa.every((r) => r.ok), sa.filter((r) => !r.ok).map((r) => JSON.stringify(r)).join(" ; ").slice(0, 800));
  L.record(F, `Apply on a scene card in a fresh window shows Changes in progress in every run (${ac.filter((r) => r.ok).length}/${ac.length})`, ac.length >= 10 && ac.every((r) => r.ok), ac.filter((r) => !r.ok).map((r) => JSON.stringify(r)).join(" ; ").slice(0, 800));
  console.log("bus at start:", before);
} catch (e) { console.log("ERROR", e); L.record(F, "T-250 loop ran", false, String(e).slice(0, 300)); } finally { await L.closeBrowser(); }
