import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F9-config-compartida";
const names = async (m) => { const s = await m.snapshot(); return [s.desk.machines.a.name, s.desk.machines.b.name]; };
const colors = async (m) => { const s = await m.snapshot(); return [s.desk.machines.a.color, s.desk.machines.b.color]; };
const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const toast = (p) => p.locator("[role=status], [aria-live], .toast").allInnerTexts();
async function closeToasts(p) { for (let i = 0; i < 6; i++) { const b = p.locator('[aria-label=Avisos] button.x'); if (!(await b.count())) break; await b.first().click({ force: true }); await L.sleep(250); } }
async function undoVisible(p) { return (await p.getByRole("button", { name: /Deshacer/ }).count()) > 0; }
try {
  const p1 = await L.openUi(m1, { hash: "#/ajustes" }), p2 = await L.openUi(m2, { hash: "#/puesto" }); await L.sleep(1500);
  const n0 = await names(m1);
  // 1. renombrar la otra máquina desde m1
  const peerName = (await m1.snapshot()).desk.machines[(await m1.snapshot()).agent === "a" ? "b" : "a"].name;
  const row = p1.locator(".row", { hasText: peerName }).first();
  await row.getByRole("button", { name: "Renombrar" }).click();
  await p1.locator("input.name").fill("Escritorio-PC"); await p1.keyboard.press("Enter"); await L.sleep(1500);
  const n1 = [await names(m1), await names(m2)];
  L.record(F, "renombrar una máquina en A llega a B (agente y tandem.toml)", eq(n1[0], n1[1]) && n1[0].includes("Escritorio-PC") && m2.toml().includes('name = "Escritorio-PC"'), JSON.stringify(n1));
  const t2 = (await L.text(p2)).replace(/\n+/g, " | ");
  L.record(F, "B muestra el aviso «Cambiado desde … · Deshacer»", /Cambiado desde/.test(t2) && (await undoVisible(p2)), t2.match(/Cambiado[^|]*\|?[^|]*/)?.[0] ?? "(sin aviso)");
  await L.shot(p2, F, "aviso-cambio");
  if (await undoVisible(p2)) { await p2.getByRole("button", { name: /Deshacer/ }).first().click(); await L.sleep(1800); }
  const n2 = [await names(m1), await names(m2)];
  L.record(F, "«Deshacer» restaura el nombre en las dos máquinas", eq(n2[0], n0) && eq(n2[1], n0), JSON.stringify({ antes: n0, despues: n2 }));
  // 2. color desde la página de la otra máquina
  const snap = await m1.snapshot(); const otherId = snap.agent === "a" ? "b" : "a";
  await p1.goto(`${m1.origin}/#/maquina/${otherId}`); await L.sleep(1200);
  const c0 = await colors(m1);
  await closeToasts(p1);
  const swatches = p1.locator("[role=radiogroup][aria-label='Color de la máquina'] [role=radio]");
  const cnt = await swatches.count(); let picked = false;
  for (let i = 0; i < cnt && !picked; i++) { const dis = await swatches.nth(i).isDisabled(); const on = (await swatches.nth(i).getAttribute("aria-checked")) === "true"; if (!dis && !on) { await swatches.nth(i).click(); picked = true; } }
  await L.sleep(1500);
  const c1 = [await colors(m1), await colors(m2)];
  L.record(F, "cambiar el color de una máquina en A llega a B", picked && eq(c1[0], c1[1]) && !eq(c1[0], c0), JSON.stringify({ antes: c0, despues: c1 }));
  // 3. atajo de bloqueo
  await p1.goto(`${m1.origin}/#/ajustes`); await L.sleep(800);
  await closeToasts(p1); await p1.getByRole("tab", { name: "Atajos" }).click(); await L.sleep(400);
  await p1.getByRole("button", { name: /Bloquear el teclado/ }).click(); await L.sleep(200);
  await p1.keyboard.press("Pause"); await L.sleep(1500);
  const lock = [m1.toml().match(/lock = \[[^\]]*\]/)?.[0], m2.toml().match(/lock = \[[^\]]*\]/)?.[0]];
  L.record(F, "cambiar el atajo de bloqueo en A llega a B", lock[0] === lock[1] && lock[0] !== 'lock = ["ScrollLock"]' && !!lock[0], JSON.stringify(lock));
  // 4. preferencias
  await closeToasts(p1); await p1.getByRole("tab", { name: "Teclado y mouse" }).click(); await L.sleep(400);
  const prefs0 = [m1.toml().match(/\[prefs\][^\[]*/)?.[0].replace(/\n/g, " ")];
  await p1.getByRole("switch", { name: "Cambiar Cmd por Ctrl" }).click().catch(async () => p1.locator("[aria-label='Cambiar Cmd por Ctrl']").click());
  await L.sleep(1500);
  const prefs1 = [m1, m2].map((m) => m.toml().match(/\[prefs\][^\[]*/)?.[0].replace(/\n/g, " "));
  L.record(F, "cambiar una preferencia (Cmd↔Ctrl) en A llega a B", prefs1[0] === prefs1[1] && prefs1[0].includes("cmd_ctrl = false"), JSON.stringify({ antes: prefs0, despues: prefs1 }));
  await closeToasts(p1); await p1.getByRole("switch", { name: "Portapapeles compartido" }).click().catch(async () => p1.locator("[aria-label='Portapapeles compartido']").click());
  await L.sleep(1500);
  const clip = [m1, m2].map((m) => /\[modules[^\]]*\]/.test(m.toml()) ? m.toml().match(/\[modules[^\[]*[\s\S]*?(?=\n\[|$)/)?.[0] : "(sin sección modules)");
  const mods = [(await m1.snapshot()).caps.here.modules.find((x) => x.id === "clipboard"), (await m2.snapshot()).caps.here.modules.find((x) => x.id === "clipboard")];
  L.record(F, "apagar «Portapapeles compartido» en A lo apaga en las dos", mods.every((m) => m.state === "off"), JSON.stringify(mods.map((m) => m.state)));
  for (const mm of ["a", "b"]) await m1.act({ kind: "set_module", machine: mm, module: "clipboard", on: true });
  // 5. conflicto simultáneo: las dos máquinas renombran a la vez la máquina A
  const a = (await m1.snapshot()).agent;
  const [q1, q2] = [await L.openUi(m1, { hash: "#/ajustes" }), await L.openUi(m2, { hash: "#/ajustes" })]; await L.sleep(1200);
  const nm1 = (await names(m1))[0]; 
  const rn = async (p, who, value) => { const r = p.locator(".row", { hasText: who }).first(); await r.getByRole("button", { name: "Renombrar" }).click(); await p.locator("input.name").fill(value); };
  await rn(q1, nm1, "Simultaneo-1"); await rn(q2, nm1, "Simultaneo-2");
  await Promise.all([q1.keyboard.press("Enter"), q2.keyboard.press("Enter")]); await L.sleep(3000);
  const sim = [await names(m1), await names(m2)];
  L.record(F, "cambio simultáneo converge al mismo valor en las dos máquinas", eq(sim[0], sim[1]) && /Simultaneo-[12]/.test(sim[0].join()), JSON.stringify(sim));
  const t3 = (await L.text(q1)).replace(/\n+/g, " | ") + (await L.text(q2)).replace(/\n+/g, " | ");
  console.log("avisos tras simultáneo:", t3.match(/Cambiado[^|]*/g));
  console.log(p1.errors, p2.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
