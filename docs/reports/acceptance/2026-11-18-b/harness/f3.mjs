import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F3-escenas";
const scenesOf = (m) => { const t = m.toml(); return [...t.matchAll(/\[\[scenes\]\]\n([^\[]*)/g)].map((x) => x[1]); };
const screenIn = async () => ({ std: [(await m2.ctl({ cmd: "monitor_screen", id: 0 })).input, (await m1.ctl({ cmd: "monitor_screen", id: 0 })).input], fail: [(await m2.ctl({ cmd: "monitor_screen", id: 2 })).input, (await m1.ctl({ cmd: "monitor_screen", id: 1 })).input] });
const pickRadio = (p, label, opt) => p.locator(".f", { hasText: `${label} muestra` }).locator("[role=radio]", { hasText: opt }).click();
try {
  const p = await L.openUi(m1, { hash: "#/monitores/escena" }); await L.sleep(1200);
  console.log("estado inicial pantallas:", JSON.stringify(await screenIn()));
  // 1. Crear
  await p.locator("input").first().fill("Prueba B");
  await pickRadio(p, "Std-34", "Mac"); 
  await p.locator("button.kbd").focus(); await p.keyboard.press("Control+Alt+9"); await L.sleep(300);
  await L.shot(p, F, "01-nueva-escena");
  await p.getByRole("button", { name: "Guardar", exact: true }).click(); await L.sleep(1500);
  const created = [m1, m2].map((m) => scenesOf(m).filter((s) => s.includes('name = "Prueba B"')));
  L.record(F, "crear escena: llega a tandem.toml de las dos máquinas", created.every((c) => c.length === 1), JSON.stringify(created[0]));
  const urlHash = await p.evaluate(() => location.hash);
  L.record(F, "crear escena: vuelve a Monitores", urlHash === "#/monitores", urlHash);
  const card = p.locator("article[aria-label='Escena Prueba B']");
  L.record(F, "crear escena: aparece como tarjeta con su atajo", (await card.count()) === 1 && /Ctrl|⌃/.test(await card.innerText()), (await card.innerText().catch(() => "")).replace(/\n/g, " | "));
  // 2. Editar
  await card.getByRole("link", { name: "Editar escena" }).click().catch(async () => card.locator("[aria-label='Editar escena']").click());
  await L.sleep(800);
  await p.locator("input").first().fill("Prueba B2");
  await pickRadio(p, "Zfail-27", "Mac");
  await p.getByRole("button", { name: "Guardar", exact: true }).click(); await L.sleep(1500);
  const edited = [m1, m2].map((m) => scenesOf(m).filter((s) => s.includes('name = "Prueba B2"')));
  L.record(F, "editar escena: el cambio llega a las dos máquinas", edited.every((c) => c.length === 1) && scenesOf(m1).every((s) => !s.includes('name = "Prueba B"\n')), JSON.stringify(edited[0]));
  // 3. Aplicar con éxito (solo el estándar): crear escena «Solo Std» y aplicarla
  await p.goto(m1.origin + "/#/monitores/escena"); await L.sleep(800);
  await p.locator("input").first().fill("Solo Std");
  await pickRadio(p, "Std-34", "Mac");
  await p.getByRole("button", { name: "Guardar y aplicar" }).click();
  const seen = new Set(); let prog = "";
  for (let i = 0; i < 24; i++) { const t = (await L.text(p)).replace(/\n+/g, " | "); const h = await p.evaluate(() => location.hash); seen.add(h); if (/Cambios en progreso/.test(t)) { prog = t; if (i % 3 === 0) await L.shot(p, F, `03-progreso-ok-${String(i).padStart(2, "0")}`); } await L.sleep(250); }
  console.log("rutas vistas tras aplicar:", [...seen], "texto de progreso:", prog.slice(150, 600));
  prog ||= "(nunca se mostró la pantalla de progreso)";
  const sc = await screenIn();
  L.record(F, "aplicar con éxito: el monitor cambió de verdad (bus del doble) y el progreso lo muestra", sc.std.includes("Hdmi") && /Cambios en progreso/.test(prog) && /confirm|listo|Listo|hecho/.test(prog), `bus=${JSON.stringify(sc)} | ${prog.slice(0, 320)}`);
  await p.goto(m1.origin + "/#/monitores"); await L.sleep(1500);
  const snap = await m1.snapshot();
  const active = (await p.locator("article[aria-label='Escena Solo Std'] .badge").count()) === 1;
  L.record(F, "aplicar con éxito: la escena queda «Activa» en la UI de las dos máquinas", active, JSON.stringify(snap.desk.active_scene ?? snap.desk.activeScene ?? Object.keys(snap.desk)));
  const p2 = await L.openUi(m2, { hash: "#/monitores" }); await L.sleep(1500);
  L.record(F, "aplicar con éxito: la otra máquina también la ve «Activa»", (await p2.locator("article[aria-label='Escena Solo Std'] .badge").count()) === 1);
  // 4. Falla inyectada: «Prueba B2» mueve Std→Mac (ya está) y Fail→Mac (el firmware rechaza). Primero pongo Std en DP con el OSD para ver la vuelta atrás.
  await m2.ctl({ cmd: "monitor_osd", id: 0, input: "dp" }); await m1.ctl({ cmd: "monitor_osd", id: 0, input: "dp" }); await L.sleep(4000);
  console.log("antes de la falla:", JSON.stringify(await screenIn()));
  await p.goto(m1.origin + "/#/monitores"); await L.sleep(1200);
  await p.locator("article[aria-label='Escena Prueba B2']").getByRole("button", { name: "Aplicar" }).click();
  await L.sleep(7000);
  const failText = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "04-vuelta-atras");
  const after = await screenIn();
  L.record(F, "falla inyectada: la UI muestra la vuelta atrás", /vuelta atr|deshac|Deshech|revirt|falló/i.test(failText), failText.slice(0, 500));
  L.record(F, "falla inyectada: el puesto vuelve a como estaba (monitores en DP, escena anterior)", after.std.every((i) => i === "DisplayPort") && after.fail.every((i) => i === "DisplayPort"), JSON.stringify(after));
  console.log(p.errors, p2.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
