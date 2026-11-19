// T-027: the "Administrator service" row of setup step 1 (This machine). On Linux the app reports
// the platform as unsupported: the row must say why, with no Install button, and invoking
// `admin_install` must be refused with the same reason (no elevation attempted).
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "ow");
const F = "T-027-admin-service-row";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
try {
  for (const m of [m1, m2]) {
    const p = await L.openUi(m, { hash: "#/settings" }); await L.sleep(1200);
    if ((await wiz(p).count()) !== 1) await p.getByRole("button", { name: /Open/ }).first().click();
    await L.until("wiz", async () => (await wiz(p).count()) === 1);
    await p.locator("aside[aria-label=Steps] button").first().click(); await L.sleep(1500);
    const main = (await wiz(p).locator("main").innerText()).replace(/\n+/g, " | ");
    const row = main.match(/Administrator service[^|]*\|?[^|]*/)?.[0] ?? "";
    const install = await wiz(p).locator("main button", { hasText: "Install" }).count();
    await L.shot(p, F, `01-step1-${m.label}`);
    const st = await m.invoke("admin_status");
    let refused = ""; try { await m.invoke("admin_install"); refused = "(accepted!)"; } catch (e) { refused = e.message; }
    const calls = await m.calls();
    console.log(m.label, JSON.stringify(st), "| install:", refused, "| row:", row);
    L.record(F, `${m.label} (${m.os} declared, Linux app): the row shows its honest unavailable reason and no Install button`, /Administrator service/.test(main) && /only exists on macOS and Windows/.test(main) && install === 0, main.slice(0, 400));
    L.record(F, `${m.label}: admin_status says off and not installable; admin_install is refused with the reason`, st.state === "off" && st.canInstall === false && /only exists on macOS and Windows/.test(refused), JSON.stringify(st) + " / " + refused + " / calls=" + JSON.stringify(calls).slice(0, 100));
    L.record(F, `${m.label}: no console errors`, p.errors.length === 0, p.errors.join(";"));
    await p.ctx.close();
  }
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
