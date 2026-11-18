import * as L from "./lib.mjs";
import { readFileSync } from "node:fs";
const { m1, m2 } = L.attach(process.argv[2] ?? "c2"); const F = "G10-editable-from-the-UI";
try {
  const p = await L.openUi(m1, { hash: "#/machine/b" }); await L.sleep(1500);
  const t = (await L.text(p)).replace(/\n+/g, " | ");
  console.log(t.slice(0, 900));
  const inputs = await p.locator("main input:visible").evaluateAll((els) => els.map((e) => [e.getAttribute("aria-label"), e.placeholder, e.value]));
  console.log(JSON.stringify(inputs));
  const wol = p.locator("main input[aria-label*='Wake' i], main input[aria-label*='MAC' i], main input[placeholder*=':']").first();
  const has = (await wol.count()) > 0;
  L.record(F, "Wake-on-LAN: the MAC is shown and edited from the other machine's page", has, has ? "" : "no field");
  if (has) {
    await wol.fill("aa:bb:cc:11:22:33");
    await p.getByRole("button", { name: /Guardar/ }).first().click().catch(() => {}); await L.sleep(1500);
    const toml = m1.toml(), toml2 = m2.toml();
    L.record(F, "Wake-on-LAN: what is typed in the UI reaches tandem.toml", /aa:bb:cc:11:22:33/i.test(toml) || /aa:bb:cc:11:22:33/i.test(toml2), (toml.match(/\[power\.wol\][^\[]*/) ?? [""])[0].replace(/\n/g, " "));
    await L.shot(p, F, "01-wol");
  }
  const p2 = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1200);
  const body0 = (await L.text(p2)).replace(/\n+/g, " | ");
  console.log(body0.slice(0, 500));
  await p2.getByRole("button", { name: "Renombrar" }).first().click(); await L.sleep(500);
  const nm = p2.locator("main input:visible, [role=dialog] input:visible").first();
  console.log("inputs:", await p2.locator("input:visible").count());
  await nm.fill("Desk Prueba"); await nm.press("Enter"); await L.sleep(1500);
  const body = await L.text(p2);
  const toml = [m1, m2].map((m) => (m.toml().match(/name = "[^"]*"/g) ?? []));
  L.record(F, "the desk name is edited from Settings and reaches the other machine", /Desk Prueba/.test(body) && toml.every((t) => t.some((x) => /Desk Prueba/.test(x))), `toml m1 ${JSON.stringify(toml[0])}; m2 ${JSON.stringify(toml[1])}`);
  await L.shot(p2, F, "02-desk-name");
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
