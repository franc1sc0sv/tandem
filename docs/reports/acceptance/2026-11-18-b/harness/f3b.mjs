import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F3-escenas";
const std = async () => [(await m2.ctl({ cmd: "monitor_screen", id: 0 })).input, (await m1.ctl({ cmd: "monitor_screen", id: 0 })).input];
const pickRadio = (p, label, opt) => p.locator(".f", { hasText: `${label} muestra` }).locator("[role=radio]", { hasText: opt }).click();
const chord = async (m, codes) => { for (const u of codes) await m.ctl({ cmd: "key", usage: u, down: true }); await L.sleep(60); for (const u of [...codes].reverse()) await m.ctl({ cmd: "key", usage: u, down: false }); };
try {
  const p = await L.openUi(m1, { hash: "#/monitores/escena" }); await L.sleep(1000);
  await p.locator("input").first().fill("Std a PC");
  await pickRadio(p, "Std-34", "PC");
  await p.locator("button.kbd").focus(); await p.keyboard.press("Control+Alt+7"); await L.sleep(200);
  await p.getByRole("button", { name: "Guardar", exact: true }).click(); await L.sleep(1200);
  // Std está en HDMI (otra escena); el atajo físico lo devuelve a DP.
  await m2.ctl({ cmd: "monitor_osd", id: 0, input: "hdmi" }); await m1.ctl({ cmd: "monitor_osd", id: 0, input: "hdmi" }); await L.sleep(4000);
  const before = await std();
  // Ctrl izq (E0), Alt izq (E2), Digit7 (0x24), en la máquina que tiene el teclado (la A), y también en la B.
  let after = before;
  for (const [who, m] of [["m1", m1], ["m2", m2]]) {
    await chord(m, [0xe0, 0xe2, 0x24]); await L.sleep(3500);
    after = await std();
    if (after.some((i, k) => i === "DisplayPort" && before[k] === "Hdmi")) { L.record(F, `atajo global de escena (Ctrl+Alt+7 en ${who}) aplica la escena`, true, `antes ${before} → después ${after}`); break; }
  }
  if (!after.some((i, k) => i === "DisplayPort" && before[k] === "Hdmi")) L.record(F, "atajo global de escena aplica la escena", false, `antes ${before} → después ${after}`);
  // El progreso se ve en la UI al aplicarla por atajo?
  const t = (await L.text(p)).replace(/\n+/g, " | ").slice(0, 300); console.log(t);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
