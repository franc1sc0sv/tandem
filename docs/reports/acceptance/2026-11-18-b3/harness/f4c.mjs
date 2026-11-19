import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host"); const F = "F4-monitors";
try {
  const snap = await m1.snapshot(); const ownerSide = snap.monitors.find((m) => m.name === "Std-34")?.owner; const ownerM = ownerSide === snap.agent ? m1 : m2;
  const p = await L.openUi(m1, { hash: "#/monitors" }); await L.sleep(1800);
  const idx = ownerM === m1 ? 0 : 0; const screen = async () => (await ownerM.ctl({ cmd: "monitor_screen", id: 0 })).input;
  const s0 = await screen(); const names = await p.locator("article[aria-label='Std-34'] [role=radio]").allInnerTexts();
  const target = s0 === "DisplayPort" ? "HDMI" : "DP";
  await p.locator("article[aria-label='Std-34'] [role=radio]", { hasText: target }).first().click(); await L.sleep(4500);
  const s1 = await screen(); const t = (await p.locator("article[aria-label='Std-34']").innerText()).replace(/\n+/g, " | ");
  await L.shot(p, F, "02-input-change");
  L.record(F, "fake standard monitor: changing the input from its card, with the monitor's verdict", s1 !== s0 && /(confirmed|Switch to)/.test(t), `bus: ${s0} → ${s1}; card: ${t.slice(0, 220)}`);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
