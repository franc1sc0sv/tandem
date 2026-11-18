import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F3-scenes";
const std = async () => [(await m2.ctl({ cmd: "monitor_screen", id: 0 })).input, (await m1.ctl({ cmd: "monitor_screen", id: 0 })).input];
const pickRadio = (p, label, opt) => p.locator(".f", { hasText: `${label} shows` }).locator("[role=radio]", { hasText: opt }).click();
const chord = async (m, codes) => { for (const u of codes) await m.ctl({ cmd: "key", usage: u, down: true }); await L.sleep(60); for (const u of [...codes].reverse()) await m.ctl({ cmd: "key", usage: u, down: false }); };
try {
  const p = await L.openUi(m1, { hash: "#/monitors/scene" }); await L.sleep(1000);
  await p.locator("input").first().fill("Std a PC");
  await pickRadio(p, "Std-34", "PC");
  await p.locator("button.kbd").focus(); await p.keyboard.press("Control+Alt+7"); await L.sleep(200);
  await p.getByRole("button", { name: "Guardar", exact: true }).click(); await L.sleep(1200);
  // Std is on HDMI (another scene); the physical shortcut returns it to DP.
  await m2.ctl({ cmd: "monitor_osd", id: 0, input: "hdmi" }); await m1.ctl({ cmd: "monitor_osd", id: 0, input: "hdmi" }); await L.sleep(4000);
  const before = await std();
  // Left Ctrl (E0), left Alt (E2), Digit7 (0x24), on the machine that has the keyboard (A), and also on B.
  let after = before;
  for (const [who, m] of [["m1", m1], ["m2", m2]]) {
    await chord(m, [0xe0, 0xe2, 0x24]); await L.sleep(3500);
    after = await std();
    if (after.some((i, k) => i === "DisplayPort" && before[k] === "Hdmi")) { L.record(F, `global scene shortcut (Ctrl+Alt+7 on ${who}) applies the scene`, true, `before ${before} → after ${after}`); break; }
  }
  if (!after.some((i, k) => i === "DisplayPort" && before[k] === "Hdmi")) L.record(F, "global scene shortcut applies the scene", false, `before ${before} → after ${after}`);
  // Is the progress visible in the UI when applied by shortcut?
  const t = (await L.text(p)).replace(/\n+/g, " | ").slice(0, 300); console.log(t);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
