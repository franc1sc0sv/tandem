import * as L from "./lib.mjs";
const F = "G2-sin-Mac-PC-fijos";
for (const [tag, banned, label] of [["win2", /\b(Mac|macOS|MacBook|⌘|⌃⌥)\b|⌘|⌃/, "Windows+Windows"], ["mac2", /\b(Windows|PC)\b/, "macOS+macOS"]]) {
  const { m1, m2 } = L.attach(tag); const hits = [];
  for (const m of [m1, m2]) {
    for (const r of ["#/puesto", "#/tuneles", "#/monitores", "#/monitores/escena", "#/acomodo", "#/red", `#/maquina/a`, `#/maquina/b`, "#/ajustes", "#/estados"]) {
      const p = await L.openUi(m, { hash: r }); await L.sleep(1200);
      const t = await L.text(p); const mt = t.match(banned);
      if (mt) hits.push(`${m.label} ${r}: «${t.slice(Math.max(0, t.search(banned) - 30), t.search(banned) + 40).replace(/\n/g, " ")}»`);
      if (p.errors.length) hits.push(`${m.label} ${r}: consola ${p.errors[0].slice(0, 120)}`);
      await p.ctx.close();
    }
    const p = await L.openUi(m, { hash: "#/puesto" }); await L.sleep(1200); await p.keyboard.press("Control+k"); await L.sleep(400);
    const cmds = (await p.locator("[role=option]").allInnerTexts()).join(" | "); if (banned.test(cmds)) hits.push(`${m.label} paleta: ${cmds.match(banned)[0]}`);
    await p.ctx.close();
  }
  console.log(label, JSON.stringify(hits, null, 1));
  L.record(F, `par ${label}: ninguna pantalla nombra el otro sistema ni usa atajos del otro sistema`, hits.length === 0, hits.slice(0, 4).join(" ; "));
}
await L.closeBrowser();
