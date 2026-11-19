// T-244 + T-270 (develop 8cda900): no "Allow remote execution" row any more; on every machine page
// (live and demo, three pairs) the only switches are Keyboard and mouse and Clipboard, and each one
// changes the module on the agent. Usage: node t270.mjs <tag>
import * as L from "./lib.mjs";
const tag = process.argv[2]; const { m1, m2 } = L.attach(tag); const F = "T-244-T-270-machine-switches"; const O = "http://127.0.0.1:38123";
const mod = async (m, id) => (await m.snapshot()).caps.here.modules.find((x) => x.id === id).state;
const me = (await m1.snapshot()).agent; const oid = me === "a" ? "b" : "a";
const p = await L.openUi(m1, { hash: `#/machine/${oid}` }); await L.sleep(2000);
const lists = {};
for (const id of [oid, me]) {
  await p.goto(`${m1.origin}/#/machine/${id}`); await L.sleep(1500);
  lists[id] = { sws: await p.locator("button[role=switch]").evaluateAll((els) => els.map((e) => e.getAttribute("aria-label"))), remote: /Allow remote execution/.test(await L.text(p)) };
  if (id === oid) await L.shot(p, F, "01-other-machine-page");
}
console.log(JSON.stringify(lists));
const two = (l) => l.sws.slice().sort().join(",") === "Clipboard,Keyboard and mouse" && !l.remote;
L.record(F, "live: no Allow remote execution row; only Keyboard and mouse and Clipboard switches on both machine pages", two(lists[oid]) && two(lists[me]), JSON.stringify(lists));
await p.goto(`${m1.origin}/#/machine/${oid}`); await L.sleep(1500);
const eff = [];
for (const [label, id] of [["Keyboard and mouse", "input"], ["Clipboard", "clipboard"]]) {
  const sw = p.locator(`button[role=switch][aria-label='${label}']`);
  const a = await mod(m2, id); await sw.click(); await L.sleep(1500); const b = await mod(m2, id); await sw.click(); await L.sleep(1500); const c = await mod(m2, id);
  eff.push(`${label}: ${a}->${b}->${c}`);
}
L.record(F, "live: each switch on the other machine's page really turns its module off and on", eff.every((x) => /active->off->active/.test(x)), eff.join("; "));
L.record(F, "live: no console errors", p.errors.length === 0, p.errors.join(";").slice(0, 200));
await p.ctx.close();
for (const q of ["?demo", "?demo=win-win", "?demo=mac-mac"]) for (const id of ["a", "b"]) {
  const pg = await L.openDemo(O, q, `#/machine/${id}`); await L.sleep(1300);
  const sws = await pg.locator("button[role=switch]").evaluateAll((els) => els.map((e) => e.getAttribute("aria-label")));
  const t = await L.text(pg);
  L.record(F, `demo ${q} machine ${id}: only the two real switches, no remote execution row, no console error`, two({ sws, remote: /Allow remote execution/.test(t) }) && pg.errors.length === 0, JSON.stringify({ sws, errors: pg.errors }));
  await pg.ctx.close();
}
await L.closeBrowser();
