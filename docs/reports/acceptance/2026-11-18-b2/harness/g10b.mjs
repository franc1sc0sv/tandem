import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "c2"); const F = "G10-editable-from-the-UI";
try {
  const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1200);
  const b = p.getByRole("button", { name: "My desk" }).first();
  console.log("My desk button:", await b.count());
  await b.click(); await L.sleep(500);
  const nm = p.locator("input:visible").first();
  await nm.fill("Taller de Pruebas"); await nm.press("Enter"); await L.sleep(1500);
  const p2 = await L.openUi(m2, { hash: "#/settings" }); await L.sleep(1500);
  const t1 = await L.text(p), t2 = await L.text(p2);
  const tomls = [m1, m2].map((m) => (m.toml().match(/(desk|desk)[a-z_]*name[^\n]*|name = "Taller[^\n]*/gi) ?? []));
  console.log(JSON.stringify(tomls), JSON.stringify((await m2.snapshot()).desk?.name ?? Object.keys((await m2.snapshot()).desk ?? {})));
  L.record(F, "the desk name is edited from Settings and the other machine sees it", /Taller de Pruebas/.test(t1) && /Taller de Pruebas/.test(t2), `m1: ${/Taller de Pruebas/.test(t1)}; m2: ${/Taller de Pruebas/.test(t2)}`);
  await L.shot(p, F, "02-desk-name-m1"); await L.shot(p2, F, "03-desk-name-m2");
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
