import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host3");
const F = "F11-modulos";
const mod = async (m, id) => (await m.snapshot()).caps.here.modules.find((x) => x.id === id);
try {
  const other = (await m1.snapshot()).agent === "a" ? m2 : m1; // la otra de m1 es m2
  const oid = (await m1.snapshot()).agent === "a" ? "b" : "a";
  const p = await L.openUi(m1, { hash: `#/maquina/${oid}` }); await L.sleep(1800);
  const sw = p.locator("button", { hasText: /^(activo|apagado)$/ });
  console.log("interruptores:", await sw.allInnerTexts());
  const row = p.locator("div,li,article", { hasText: /^Teclado y mouse/ }).last();
  const b0 = await mod(m2, "input"); 
  // apagar «Portapapeles»/«Monitores» de la otra desde su página
  const clipRow = p.locator("div", { hasText: "Texto, HTML e imagen" }).locator("button", { hasText: /activo|apagado/ }).last();
  const c0 = (await mod(m2, "clipboard")).state;
  await clipRow.click(); await L.sleep(1500);
  const c1 = (await mod(m2, "clipboard")).state; const c1here = (await mod(m1, "clipboard")).state;
  L.record(F, "encender/apagar un módulo de la otra máquina desde su página (Portapapeles)", c0 !== c1, `otra: ${c0} → ${c1}; (esta: ${c1here})`);
  await clipRow.click(); await L.sleep(1500);
  const inRow = sw.nth(0);
  const i0 = (await mod(m2, "input")).state; await inRow.click(); await L.sleep(1500); const i1 = (await mod(m2, "input")).state; await inRow.click(); await L.sleep(1500); const i2 = (await mod(m2, "input")).state;
  L.record(F, "apagar y volver a encender «Teclado y mouse» de la otra máquina", i0 === "active" && i1 === "off" && i2 === "active", `${i0} → ${i1} → ${i2}`);
  const mods = (await m2.snapshot()).caps.here.modules.map((x) => `${x.id}:${JSON.stringify(x.state)}`);
  const txt = (await L.text(p)).replace(/\n+/g, " | ");
  const tun = (await mod(m2, "tunnels")), fil = (await mod(m2, "files"));
  // los túneles sí funcionan (F5) aunque el módulo diga otra cosa
  L.record(F, "razones de «no disponible» correctas: Túneles no dice «aún no está» si ya funciona", tun.state !== "unavailable" || !/Aún no está/.test(tun.reason ?? ""), `Túneles: ${tun.state} · «${tun.reason}» (los túneles funcionaron en F5)`);
  L.record(F, "razones de «no disponible» correctas: Archivos (arrastrar archivos, T-121)", fil.state !== "unavailable" || !/Aún no está/.test(fil.reason ?? ""), `Archivos: ${fil.state} · «${fil.reason}»`);
  console.log(mods);
  await L.shot(p, F, "01-pagina-maquina");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
