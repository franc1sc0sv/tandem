import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "c2"); const F = "G10-editable-desde-la-UI";
try {
  const p = await L.openUi(m1, { hash: "#/ajustes" }); await L.sleep(1200);
  const b = p.getByRole("button", { name: "Mi puesto" }).first();
  console.log("botón Mi puesto:", await b.count());
  await b.click(); await L.sleep(500);
  const nm = p.locator("input:visible").first();
  await nm.fill("Taller de Pruebas"); await nm.press("Enter"); await L.sleep(1500);
  const p2 = await L.openUi(m2, { hash: "#/ajustes" }); await L.sleep(1500);
  const t1 = await L.text(p), t2 = await L.text(p2);
  const tomls = [m1, m2].map((m) => (m.toml().match(/(desk|puesto)[a-z_]*name[^\n]*|name = "Taller[^\n]*/gi) ?? []));
  console.log(JSON.stringify(tomls), JSON.stringify((await m2.snapshot()).desk?.name ?? Object.keys((await m2.snapshot()).desk ?? {})));
  L.record(F, "el nombre del puesto se edita desde Ajustes y la otra máquina lo ve", /Taller de Pruebas/.test(t1) && /Taller de Pruebas/.test(t2), `m1: ${/Taller de Pruebas/.test(t1)}; m2: ${/Taller de Pruebas/.test(t2)}`);
  await L.shot(p, F, "02-nombre-puesto-m1"); await L.shot(p2, F, "03-nombre-puesto-m2");
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
