import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F3-scenes";
const scenesOf = (m) => { const t = m.toml(); return [...t.matchAll(/\[\[scenes\]\]\n([^\[]*)/g)].map((x) => x[1]); };
const screenIn = async () => ({ std: [(await m2.ctl({ cmd: "monitor_screen", id: 0 })).input, (await m1.ctl({ cmd: "monitor_screen", id: 0 })).input], fail: [(await m2.ctl({ cmd: "monitor_screen", id: 2 })).input, (await m1.ctl({ cmd: "monitor_screen", id: 1 })).input] });
const pickRadio = (p, label, opt) => p.locator(".f", { hasText: `${label} shows` }).locator("[role=radio]", { hasText: opt }).click();
try {
  const p = await L.openUi(m1, { hash: "#/monitors/scene" }); await L.sleep(1200);
  console.log("state initial screens:", JSON.stringify(await screenIn()));
  // 1. Crear
  await p.locator("input").first().fill("Prueba B");
  await pickRadio(p, "Std-34", "Mac"); 
  await p.locator("button.kbd").focus(); await p.keyboard.press("Control+Alt+9"); await L.sleep(300);
  await L.shot(p, F, "01-new-scene");
  await p.getByRole("button", { name: "Guardar", exact: true }).click(); await L.sleep(1500);
  const created = [m1, m2].map((m) => scenesOf(m).filter((s) => s.includes('name = "Prueba B"')));
  L.record(F, "create scene: reaches tandem.toml on both machines", created.every((c) => c.length === 1), JSON.stringify(created[0]));
  const urlHash = await p.evaluate(() => location.hash);
  L.record(F, "create scene: returns to Monitors", urlHash === "#/monitors", urlHash);
  const card = p.locator("article[aria-label='Scene Prueba B']");
  L.record(F, "create scene: appears as a card with its shortcut", (await card.count()) === 1 && /Ctrl|⌃/.test(await card.innerText()), (await card.innerText().catch(() => "")).replace(/\n/g, " | "));
  // 2. Edit
  await card.getByRole("link", { name: "Edit scene" }).click().catch(async () => card.locator("[aria-label='Edit scene']").click());
  await L.sleep(800);
  await p.locator("input").first().fill("Prueba B2");
  await pickRadio(p, "Zfail-27", "Mac");
  await p.getByRole("button", { name: "Guardar", exact: true }).click(); await L.sleep(1500);
  const edited = [m1, m2].map((m) => scenesOf(m).filter((s) => s.includes('name = "Prueba B2"')));
  L.record(F, "edit scene: the change reaches both machines", edited.every((c) => c.length === 1) && scenesOf(m1).every((s) => !s.includes('name = "Prueba B"\n')), JSON.stringify(edited[0]));
  // 3. Apply with success (standard only): create scene "Only Std" and apply it
  await p.goto(m1.origin + "/#/monitors/scene"); await L.sleep(800);
  await p.locator("input").first().fill("Solo Std");
  await pickRadio(p, "Std-34", "Mac");
  await p.getByRole("button", { name: "Guardar y apply" }).click();
  const seen = new Set(); let prog = "";
  for (let i = 0; i < 24; i++) { const t = (await L.text(p)).replace(/\n+/g, " | "); const h = await p.evaluate(() => location.hash); seen.add(h); if (/Changes in progress/.test(t)) { prog = t; if (i % 3 === 0) await L.shot(p, F, `03-progress-ok-${String(i).padStart(2, "0")}`); } await L.sleep(250); }
  console.log("routes views after apply:", [...seen], "text de progress:", prog.slice(150, 600));
  prog ||= "(the progress screen was never shown)";
  const sc = await screenIn();
  L.record(F, "apply with success: the monitor really changed (double's bus) and progress shows it", sc.std.includes("Hdmi") && /Changes in progress/.test(prog) && /confirm|done|Done/.test(prog), `bus=${JSON.stringify(sc)} | ${prog.slice(0, 320)}`);
  await p.goto(m1.origin + "/#/monitors"); await L.sleep(1500);
  const snap = await m1.snapshot();
  const active = (await p.locator("article[aria-label='Scene Solo Std'] .badge").count()) === 1;
  L.record(F, "apply with success: the scene shows as \"Active\" in the UI of both machines", active, JSON.stringify(snap.desk.active_scene ?? snap.desk.activeScene ?? Object.keys(snap.desk)));
  const p2 = await L.openUi(m2, { hash: "#/monitors" }); await L.sleep(1500);
  L.record(F, "apply with success: the other machine also sees it as \"Active\"", (await p2.locator("article[aria-label='Scene Solo Std'] .badge").count()) === 1);
  // 4. Injected failure: "Test B2" moves Std→Mac (already there) and Fail→Mac (the firmware rejects). First I put Std on DP with the OSD to see the rollback.
  await m2.ctl({ cmd: "monitor_osd", id: 0, input: "dp" }); await m1.ctl({ cmd: "monitor_osd", id: 0, input: "dp" }); await L.sleep(4000);
  console.log("before the failure:", JSON.stringify(await screenIn()));
  await p.goto(m1.origin + "/#/monitors"); await L.sleep(1200);
  await p.locator("article[aria-label='Scene Prueba B2']").getByRole("button", { name: "Apply" }).click();
  await L.sleep(7000);
  const failText = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "04-rollback");
  const after = await screenIn();
  L.record(F, "injected failure: the UI shows the rollback", /rollback|undo|Undone|revert|failed/i.test(failText), failText.slice(0, 500));
  L.record(F, "injected failure: the desk returns to how it was (monitors on DP, previous scene)", after.std.every((i) => i === "DisplayPort") && after.fail.every((i) => i === "DisplayPort"), JSON.stringify(after));
  console.log(p.errors, p2.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
