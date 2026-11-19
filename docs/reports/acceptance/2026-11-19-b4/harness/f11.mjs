import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host3");
const F = "F11-modules";
const mod = async (m, id) => (await m.snapshot()).caps.here.modules.find((x) => x.id === id);
try {
  const other = (await m1.snapshot()).agent === "a" ? m2 : m1; // the other one of m1 is m2
  const oid = (await m1.snapshot()).agent === "a" ? "b" : "a";
  const p = await L.openUi(m1, { hash: `#/machine/${oid}` }); await L.sleep(1800);
  const sw = p.locator("button[role=switch].tog");
  console.log("switches:", await sw.allInnerTexts());
  const row = p.locator("div,li,article", { hasText: /^Keyboard and mouse/ }).last();
  const b0 = await mod(m2, "input"); 
  // turn off "Clipboard"/"Monitors" of the other machine from its page
  const clipRow = p.locator("button[role=switch][aria-label=Clipboard]");
  const c0 = (await mod(m2, "clipboard")).state;
  await clipRow.click(); await L.sleep(1500);
  const c1 = (await mod(m2, "clipboard")).state; const c1here = (await mod(m1, "clipboard")).state;
  L.record(F, "turn a module of the other machine on/off from its page (Clipboard)", c0 !== c1, `other: ${c0} → ${c1}; (this: ${c1here})`);
  await clipRow.click(); await L.sleep(1500);
  const inRow = p.locator("button[role=switch][aria-label=\"Keyboard and mouse\"]");
  const i0 = (await mod(m2, "input")).state; await inRow.click(); await L.sleep(1500); const i1 = (await mod(m2, "input")).state; await inRow.click(); await L.sleep(1500); const i2 = (await mod(m2, "input")).state;
  L.record(F, "turn \"Keyboard and mouse\" of the other machine off and on again", i0 === "active" && i1 === "off" && i2 === "active", `${i0} → ${i1} → ${i2}`);
  const mods = (await m2.snapshot()).caps.here.modules.map((x) => `${x.id}:${JSON.stringify(x.state)}`);
  const txt = (await L.text(p)).replace(/\n+/g, " | ");
  const tun = (await mod(m2, "tunnels")), fil = (await mod(m2, "files"));
  // tunnels do work (F5) even if the module says otherwise
  L.record(F, "correct \"unavailable\" reasons: Tunnels does not say \"not yet\" if it already works", tun.state !== "unavailable" || !/Not yet/.test(tun.reason ?? ""), `Tunnels: ${tun.state} · «${tun.reason}» (tunnels worked in F5)`);
  L.record(F, "correct \"unavailable\" reasons: Files (file drag, T-121)", fil.state !== "unavailable" || !/Not yet/.test(fil.reason ?? ""), `Files: ${fil.state} · «${fil.reason}»`);
  console.log(mods);
  await L.shot(p, F, "01-machine-page");
  console.log(p.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
