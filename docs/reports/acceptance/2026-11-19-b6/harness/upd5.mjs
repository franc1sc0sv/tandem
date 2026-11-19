// T-251: "Update all" in Settings -> Updates. Samples the pane every 100 ms from the click until
// the run ends (the other machine never comes back in this double: 20 s timeout). Checks that the
// step text shows right away and while the bridge has a step, "Update all" stays disabled
// meanwhile, a second click does nothing, it is enabled again with the error at the end, the
// other agent got the request, and there is no console error. Repeats R times (default 3).
import * as L from "./lib.mjs"; import http from "node:http";
const tag = process.argv[2] ?? "fin"; const R = Number(process.argv[3] ?? 3); const { m1, m2 } = L.attach(tag); const F = "T-251-update-all";
const srv = http.createServer((q, r) => { const b = JSON.stringify({ version: "9.9.9" }); r.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(b), connection: "close" }); r.end(b); }).listen(0, "127.0.0.1");
await new Promise((r) => srv.on("listening", r)); const port = srv.address().port;
try { process.kill(m1.bridgePid, "SIGKILL"); } catch {} await L.sleep(500); await m1.startBridge(["--latest-json", `http://127.0.0.1:${port}/latest.json`]); L.saveInfo(tag, m1, m2);
const STEP = /Asking the other machine to update…|The other machine is updating; this one keeps the keyboard and mouse…|Installing on this machine/;
try {
  for (let r = 1; r <= R; r++) {
    const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1200); await p.getByRole("tab", { name: "Updates" }).click(); await L.sleep(800);
    await p.getByRole("button", { name: /Check now|Check again/ }).click(); await L.sleep(1500);
    const logBefore = m2.agentLog().length;
    const t0 = Date.now();
    await p.getByRole("button", { name: "Update all" }).click();
    const samples = []; let secondClick = null;
    for (let i = 0; i < 300; i++) {
      const [st, pane, enabled] = await Promise.all([m1.invoke("update_get"), p.locator("#pane").innerText(), p.getByRole("button", { name: "Update all" }).isEnabled().catch(() => null)]);
      samples.push({ t: Date.now() - t0, step: st.step, seq: st.seq, err: st.error, shown: (pane.match(STEP) ?? [null])[0], enabled });
      if (i === 15 && secondClick === null) { await p.getByRole("button", { name: "Update all" }).click({ force: true, timeout: 1000 }).catch(() => {}); secondClick = (await m1.invoke("update_get")).seq; }
      if (i === 20 && r === 1) await L.shot(p, F, "01-update-all-running");
      if (st.step === null && i > 5 && !st.busy) { if (samples.filter((s) => s.step === null).length > 3) break; }
      await L.sleep(100);
    }
    await L.sleep(500);
    const end = (await p.locator("#pane").innerText()).replace(/\n+/g, " | ");
    if (r === 1) await L.shot(p, F, "02-update-all-ended");
    const running = samples.filter((s) => s.step !== null);
    const firstShown = samples.find((s) => s.shown);
    const mismatch = running.filter((s) => s.t >= 300 && (!s.shown || s.enabled !== false)); // the first ~100 ms are the click round trip (the bridge has the step before the reply reaches the window)
    const finalEnabled = await p.getByRole("button", { name: "Update all" }).isEnabled().catch(() => null);
    const peerLog = m2.agentLog().slice(logBefore).replace(/\x1b\[[0-9;]*m/g, "");
    const asked = /update|install/i.test(peerLog);
    const errs = p.errors.slice();
    console.log(`run ${r}: steps`, [...new Set(running.map((s) => s.step))], "first shown at", firstShown?.t, "ms; running samples", running.length, "mismatches", mismatch.length, "end:", end.slice(0, 260), "final enabled", finalEnabled, "errors", errs);
    L.record(F, `run ${r}: the step shows within 300 ms of the click and in every sample while the run has a step`, !!firstShown && firstShown.t < 300 && running.length > 50 && mismatch.length === 0, `first shown at ${firstShown?.t} ms; ${running.length} running samples; mismatches: ${JSON.stringify(mismatch.slice(0, 3))}`);
    L.record(F, `run ${r}: Update all disabled while running; a second click changes nothing`, running.every((s) => s.enabled === false) && samples.filter((s) => s.err === "an update is already in progress").length === 0 && !/already in progress/.test(end), `second click at seq ${secondClick}; end: ${end.slice(0, 200)}`);
    L.record(F, `run ${r}: at the end the error is shown and Update all is enabled again`, /did not come back/.test(end) && finalEnabled === true, end.slice(0, 260));
    L.record(F, `run ${r}: the other agent received the request`, asked, peerLog.split("\n").filter((l) => /update|install/i.test(l)).slice(0, 2).join(" ").slice(0, 240));
    L.record(F, `run ${r}: no console error`, errs.length === 0, errs.join(" ; ").slice(0, 300));
    await p.ctx.close();
  }
} catch (e) { console.log("ERROR", e); L.record(F, "T-251 script ran", false, String(e).slice(0, 300)); } finally { srv.close(); await L.closeBrowser(); process.exit(0); }
