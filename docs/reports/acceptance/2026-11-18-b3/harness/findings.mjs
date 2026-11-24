// Deterministic findings against the real agent (each with its evidence).
import * as L from "./lib.mjs"; import { readFileSync } from "node:fs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host3");
try {
  // F2/P6: the live crossing test
  { const F = "F2-live-crossing-test";
    const p = await L.openUi(m1, { hash: "#/desk" }); await L.sleep(1200); await p.locator("nav a, aside a", { hasText: "Arrangement" }).first().click(); await L.sleep(1200);
    const dis = await p.getByRole("link", { name: /Test crossing/ }).or(p.getByRole("button", { name: /Test crossing/ })).first().getAttribute("aria-disabled").catch(() => null);
    const disabled = await p.locator("a:has-text('Test crossing'), button:has-text('Test crossing')").first().evaluate((e) => e.disabled === true || e.getAttribute("aria-disabled") === "true" || e.hasAttribute("disabled"));
    L.record(F, "Arrangement: \"Test crossing\" is available with the real agent", !disabled, disabled ? "the button is disabled (store.can('crossing') always false in LiveSource)" : "enabled");
    await L.shot(p, F, "01-arrangement-test-crossing-disabled");
    await p.goto(m1.origin + "/#/arrangement/crossing"); await L.sleep(2500); const t = (await L.text(p)).replace(/\n+/g, " | ");
    L.record(F, "crossing test with a real CursorSample from the engine: it can be completed", /Crossed OK/.test(t), t.slice(40, 300));
    await L.shot(p, F, "02-live-crossing-test");
    await p.ctx.close();
    // the engine does report the cursor: watch_cursor gives real samples
    const r = await m1.act({ kind: "watch_cursor", on: true }); L.record(F, "the agent does accept watch_cursor (what is missing is the UI)", r.kind === "done", JSON.stringify(r)); await m1.act({ kind: "watch_cursor", on: false });
  }
  // N4: can() is not reactive: reloading on the screen leaves buttons disabled
  { const F = "G6-buttons-with-effect";
    const p = await L.openUi(m1, { hash: "#/network" }); await L.sleep(3000);
    const dis = await p.getByRole("button", { name: /Medir width de band/ }).isDisabled();
    L.record(F, "reloading the window on Network leaves \"Measure bandwidth\" enabled", !dis, dis ? "stays disabled forever (can() is not reactive)" : "ok");
    await p.ctx.close(); }
  // Tunnels on open
  { const F = "F5-tunnels-mcp"; const { Mcp } = await import("./mcp.mjs");
    const c = new Mcp(m1, "/tmp/claude-1000/acc3b"); await c.init(); const t = await c.call("tunnel_open", { title: "for-the-ui" });
    await L.sleep(1500); const p = await L.openUi(m1, { hash: "#/tunnels" }); await L.sleep(3000); const tx = (await L.text(p)).replace(/\n+/g, " | ");
    L.record(F, "a UI opened (or reloaded) after opening a tunnel shows it without waiting for another change", /for-the-ui/.test(tx), tx.slice(60, 220));
    await c.call("tunnel_close", { tunnel: t.value.id }); c.close(); await p.ctx.close(); }
  // Setup: "Close"
  { const F = "F1-setup"; const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1000); await p.getByRole("button", { name: /Open/ }).first().click(); await L.sleep(1500);
    const b = await p.locator(".wz-close").boundingBox(); const hit = await p.evaluate(() => { const e = document.querySelector(".wz-close").getBoundingClientRect(); return document.elementFromPoint(e.x + e.width / 2, e.y + e.height / 2)?.className; });
    L.record(F, "the setup \"Close\" button is visible and can be pressed", b.height >= 24 && /wz-close/.test(String(hit)), `box ${Math.round(b.width)}×${Math.round(b.height)} px; what receives the click: ${hit}`);
    await L.shot(p, F, "00-close-collapsed"); await p.ctx.close(); }
  // Machine: "unavailable" rules
  { const F = "F1-setup"; const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(800); await p.getByRole("button", { name: /Open/ }).first().click(); await L.sleep(800); await p.keyboard.press("Escape");
    await p.locator("body").click({ position: { x: 5, y: 5 } }).catch(() => {}); await p.evaluate(() => { location.hash = "#/desk"; });
    await p.goto(m1.origin + "/#/settings"); await p.getByRole("button", { name: /Open/ }).first().click(); await L.sleep(500); await p.locator("aside[aria-label=Steps] button", { hasText: "Done" }).click(); await L.sleep(800);
    const t = (await p.locator('[role=dialog][aria-label=Setup] main').innerText()).replace(/\n+/g, " | ");
    L.record(F, "final summary: \"Remote compute\" reflects that tunnels work", !/remote compute is missing/.test(t), t.slice(0, 340)); await L.shot(p, F, "06-compute-summary"); await p.ctx.close(); }
  // Setup step 1: Files and Compute
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
