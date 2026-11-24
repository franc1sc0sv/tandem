// T-240: the key probe of setup step 4 must reach the other window even if the key is heard on a
// machine whose own link is not installed yet. Pair where `blocked` cannot dial (its link only
// appears once it adopts the other's connection): restart both, open step 4 on both windows at once,
// press a key on the lagging machine before its link is up, and check the other window marks it.
// Usage: node t240.mjs <tag> <m1|m2>
import * as L from "./lib.mjs";
const tag = process.argv[2] ?? "ow"; const blocked = process.argv[3] ?? "m2";
const F = "T-240-key-probe-replay";
const { m1, m2 } = L.attach(tag);
const [lag, fast] = blocked === "m1" ? [m1, m2] : [m2, m1];
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Steps] button", { hasText: s }).click();
const grp = (p, n) => wiz(p).locator(`[role=group][aria-label="${n}"]`).innerText();
const has = (t) => /Activity detected/.test(t);
try {
  // Open both windows on step 4 while the link is up, so the probe is armed, then restart the lagging agent.
  const pf = await L.openUi(fast, { hash: "#/settings" }); await L.sleep(1200); if ((await wiz(pf).count()) !== 1) await pf.getByRole("button", { name: /Open/ }).first().click();
  const pl = await L.openUi(lag, { hash: "#/settings" }); await L.sleep(1200); if ((await wiz(pl).count()) !== 1) await pl.getByRole("button", { name: /Open/ }).first().click();
  for (const p of [pf, pl]) { await L.until("wiz", async () => (await wiz(p).count()) === 1); await step(p, "Keyboard and mouse"); }
  await L.sleep(1500);
  const lagName = (await lag.snapshot()).desk.machines[(await lag.snapshot()).agent].name;
  L.record(F, "baseline: the lagging machine's row is not marked in the other window", !has(await grp(pf, lagName)), lagName);
  lag.kill("SIGTERM"); await L.sleep(1500); lag.startAgent();
  const t0 = Date.now(); let ipc = false, linkAtPress = null;
  // Press as soon as the lagging agent's IPC answers and the other side already sees it again, but before its own link is installed.
  for (let i = 0; i < 300; i++) {
    try { const s = await lag.snapshot(); ipc = true; const fs = await fast.snapshot(); if (fs.peer.link && !s.peer.link) { linkAtPress = false; break; } if (s.peer.link) { linkAtPress = true; break; } } catch {}
    await L.sleep(50);
  }
  const armed = "replayed by the other side";
  const hb = await lag.act({ kind: "heard_key" }).catch((e) => ({ error: e.message }));
  const linkNow = !!(await lag.snapshot()).peer.link;
  console.log(`ipc=${ipc} lagging link when the key was heard: ${linkAtPress === false ? "not yet installed" : linkAtPress ? "already up" : "unknown"} (now ${linkNow}); ${Date.now() - t0} ms after restart; heard_key=${JSON.stringify(hb)} arm=${JSON.stringify(armed)}`);
  const ok = await L.until("other window marks it", async () => has(await grp(pf, lagName)), 15000).then(() => true, () => false);
  await L.shot(pf, F, "01-other-window");
  L.record(F, "a key heard on the machine whose link is not up yet reaches the other window once its link appears", ok, `link when heard: ${linkAtPress === false ? "not installed" : "up (race not forced)"}; ${(await grp(pf, lagName)).replace(/\n/g, " | ")}`);
  console.log(pf.errors, pl.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
