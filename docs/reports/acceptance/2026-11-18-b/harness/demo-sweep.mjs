import * as L from "./lib.mjs";
const O = "http://127.0.0.1:38123";
const F = "DEMO-barrido";
const routes = ["#/puesto", "#/tuneles", "#/monitores", "#/monitores/escena", "#/acomodo", "#/acomodo/cruce", "#/red", "#/maquina/a", "#/maquina/b", "#/ajustes", "#/estados", "#/progreso"];
const variants = [["?demo", "mac-win"], ["?demo=win-win", "win-win"], ["?demo=mac-mac", "mac-mac"]];
const bad = /\{\{|undefined|NaN|\[object|null\b/;
const english = /\b(Loading|Error|Cancel|Save|Settings|Delete|Machine|Please|Retry|Connect|Disconnect|Unavailable|Click|Apply|Close|Open)\b/;
const problems = [];
for (const [q, name] of variants) for (const r of routes) {
  const p = await L.openDemo(O, q, r); await L.sleep(900);
  const t = await L.text(p);
  if (bad.test(t)) problems.push(`${name} ${r}: texto con ${t.match(bad)[0]} → «${t.slice(Math.max(0, t.search(bad) - 40), t.search(bad) + 40).replace(/\n/g, " ")}»`);
  const en = t.match(english); if (en) problems.push(`${name} ${r}: posible inglés «${en[0]}»`);
  if (p.errors.length) problems.push(`${name} ${r}: consola ${p.errors.join(" / ").slice(0, 200)}`);
  if (/tercera|Teclado aquí|requiere cable/i.test(t)) problems.push(`${name} ${r}: texto prohibido`);
  await p.ctx.close();
}
for (const [q, name] of [["?demo&trouble=no-agent", "no-agent"], ["?demo&trouble=peer-off", "peer-off"], ["?demo&trouble=link-down", "link-down"], ["?demo&trouble=permissions", "permissions"], ["?demo&txn=run#/progreso", "txn-run"], ["?demo&txn=ok", "txn-ok"], ["?demo&txn=fail", "txn-fail"], ["?demo&cross=ok", "cross-ok"], ["?demo&cross=wrong", "cross-wrong"], ["?demo&cross=timeout", "cross-timeout"], ["?demo&tunnels=none", "tun-none"], ["?demo&tunnels=many", "tun-many"], ["?demo&avisos", "avisos"], ["?demo&setup", "setup"]]) {
  const [qq, h] = q.split("#"); const hash = h ? "#" + h : name.startsWith("cross") ? "#/acomodo/cruce" : name.startsWith("tun") ? "#/tuneles" : name.startsWith("txn") ? "#/progreso" : "";
  const p = await L.openDemo(O, qq, hash); await L.sleep(1500);
  const t = await L.text(p);
  if (bad.test(t)) problems.push(`${name}: texto con ${t.match(bad)[0]} → «${t.slice(Math.max(0, t.search(bad) - 40), t.search(bad) + 40).replace(/\n/g, " ")}»`);
  if (p.errors.length) problems.push(`${name}: consola ${p.errors.join(" / ").slice(0, 200)}`);
  await p.ctx.close();
}
console.log(JSON.stringify(problems, null, 1));
L.record(F, "modo demo: ninguna consola con errores, ningún {{ undefined NaN ni inglés en todas las pantallas y variantes", problems.length === 0, problems.join(" ; ").slice(0, 600));
await L.closeBrowser();
