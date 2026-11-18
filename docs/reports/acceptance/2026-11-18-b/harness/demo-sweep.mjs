import * as L from "./lib.mjs";
const O = "http://127.0.0.1:38123";
const F = "DEMO-sweep";
const routes = ["#/desk", "#/tunnels", "#/monitors", "#/monitors/scene", "#/arrangement", "#/arrangement/crossing", "#/network", "#/machine/a", "#/machine/b", "#/settings", "#/states", "#/progress"];
const variants = [["?demo", "mac-win"], ["?demo=win-win", "win-win"], ["?demo=mac-mac", "mac-mac"]];
const bad = /\{\{|undefined|NaN|\[object|null\b/;
const spanish = /\b(Cargando|Cancelar|Guardar|Eliminar|Reintentar|Conectar|Desconectar|Apply|Cerrar|Open|Desk|Settings)\b/;
const problems = [];
for (const [q, name] of variants) for (const r of routes) {
  const p = await L.openDemo(O, q, r); await L.sleep(900);
  const t = await L.text(p);
  if (bad.test(t)) problems.push(`${name} ${r}: text with ${t.match(bad)[0]} → «${t.slice(Math.max(0, t.search(bad) - 40), t.search(bad) + 40).replace(/\n/g, " ")}»`);
  const es = t.match(spanish); if (es) problems.push(`${name} ${r}: possible Spanish text «${es[0]}»`);
  if (p.errors.length) problems.push(`${name} ${r}: console ${p.errors.join(" / ").slice(0, 200)}`);
  if (/third machine|Keyboard here|requires cable/i.test(t)) problems.push(`${name} ${r}: forbidden text`);
  await p.ctx.close();
}
for (const [q, name] of [["?demo&trouble=no-agent", "no-agent"], ["?demo&trouble=peer-off", "peer-off"], ["?demo&trouble=link-down", "link-down"], ["?demo&trouble=permissions", "permissions"], ["?demo&txn=run#/progress", "txn-run"], ["?demo&txn=ok", "txn-ok"], ["?demo&txn=fail", "txn-fail"], ["?demo&cross=ok", "cross-ok"], ["?demo&cross=wrong", "cross-wrong"], ["?demo&cross=timeout", "cross-timeout"], ["?demo&tunnels=none", "tun-none"], ["?demo&tunnels=many", "tun-many"], ["?demo&notices", "notices"], ["?demo&setup", "setup"]]) {
  const [qq, h] = q.split("#"); const hash = h ? "#" + h : name.startsWith("cross") ? "#/arrangement/crossing" : name.startsWith("tun") ? "#/tunnels" : name.startsWith("txn") ? "#/progress" : "";
  const p = await L.openDemo(O, qq, hash); await L.sleep(1500);
  const t = await L.text(p);
  if (bad.test(t)) problems.push(`${name}: text with ${t.match(bad)[0]} → «${t.slice(Math.max(0, t.search(bad) - 40), t.search(bad) + 40).replace(/\n/g, " ")}»`);
  if (p.errors.length) problems.push(`${name}: console ${p.errors.join(" / ").slice(0, 200)}`);
  await p.ctx.close();
}
console.log(JSON.stringify(problems, null, 1));
L.record(F, "demo mode: no console errors, no {{ undefined NaN or Spanish text on any screen or variant", problems.length === 0, problems.join(" ; ").slice(0, 600));
await L.closeBrowser();
