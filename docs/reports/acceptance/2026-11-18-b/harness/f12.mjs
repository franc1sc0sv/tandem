import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { readFileSync, existsSync } from "node:fs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F12-ajustes-paleta";
const steps = (m) => existsSync(m.dataDir + "/sim-steps.jsonl") ? readFileSync(m.dataDir + "/sim-steps.jsonl", "utf8").trim().split("\n").filter(Boolean).length : 0;
const entered = async (src, dst, [x, y, dx, dy], tries) => { for (let i = 0; i < tries; i++) { const before = (await dst.ctl({ cmd: "injected" })).count; await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90); const r = await dst.ctl({ cmd: "injected" }); if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true; } return false; };
async function palette(p, query) { await p.keyboard.press("Control+k"); await L.sleep(400); const inp = p.locator("[role=dialog] input, [role=combobox], input[type=text], input").last(); await inp.fill(query); await L.sleep(300); }
async function runCmd(p, query) { await palette(p, query); const opts = await p.locator("[role=option]").allInnerTexts(); await p.keyboard.press("Enter"); await L.sleep(800); return opts; }
try {
  const p = await L.openUi(m1, { hash: "#/puesto" }); await L.sleep(1800);
  await p.keyboard.press("Control+k"); await L.sleep(500);
  const all = (await p.locator("[role=option]").allInnerTexts()).map((x) => x.replace(/\n/g, " ")); console.log("COMANDOS:", all);
  await L.shot(p, F, "01-paleta"); await p.keyboard.press("Escape"); await L.sleep(300);
  const wantTitles = ["Aplicar", "Nueva escena", "Apagar", "Cambiar la otra máquina", "Monitores", "Red", "Acomodo", "Ajustes", "Puesto", "Túneles", "Medir el enlace", "Bloquear aquí", "Quitar el bloqueo", "Abrir el asistente"];
  L.record(F, "la paleta lista todos los comandos de D15", wantTitles.every((w) => all.some((a) => a.includes(w))), "faltan: " + wantTitles.filter((w) => !all.some((a) => a.includes(w))).join(","));
  // navegación
  for (const [q, hash] of [["Monitores", "#/monitores"], ["Red", "#/red"], ["Acomodo", "#/acomodo"], ["Ajustes", "#/ajustes"], ["Túneles", "#/tuneles"], ["Puesto", "#/puesto"]]) {
    await runCmd(p, q); const h = await p.evaluate(() => location.hash); L.record(F, `paleta «${q}» navega a ${hash}`, h === hash, h);
  }
  await p.keyboard.press("Control+,"); await L.sleep(500); L.record(F, "Ctrl+, abre Ajustes", (await p.evaluate(() => location.hash)) === "#/ajustes");
  // asistente
  await runCmd(p, "Abrir el asistente"); L.record(F, "paleta «Abrir el asistente» lo abre", (await p.locator('[role=dialog][aria-label=Asistente]').count()) === 1); await p.keyboard.press("Escape"); await L.sleep(500);
  await runCmd(p, "Cambiar la otra máquina"); const w1 = await p.locator('[role=dialog][aria-label=Asistente] [aria-current=step]').innerText().catch(() => ""); L.record(F, "paleta «Cambiar la otra máquina…» abre el asistente en «Tu puesto»", /Tu puesto/.test(w1), w1); await p.keyboard.press("Escape"); await L.sleep(500);
  // medir
  await runCmd(p, "Medir el enlace"); await L.sleep(7000);
  const net = (await p.evaluate(() => location.hash)); const txt = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "paleta «Medir el enlace» pide la medición al agente y la UI da el valor o la razón", /Mbps|Gbps|Midiendo|ancho de banda|No se mide/i.test(txt), `${net} ${txt.slice(60, 300)}`);
  await p.goto(m1.origin + "/#/puesto"); await L.sleep(1500);
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
