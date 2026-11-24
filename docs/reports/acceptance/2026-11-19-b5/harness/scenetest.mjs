import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const out = [];
const owner = async () => (await m1.snapshot()).monitors.find((m) => m.name === "Std-34")?.owner;
const bus = async () => [(await m1.ctl({ cmd: "monitor_screen", id: 0 })).input, (await m2.ctl({ cmd: "monitor_screen", id: 0 })).input];
try {
  console.log("sides: m1=", (await m1.snapshot()).agent, " m2=", (await m2.snapshot()).agent, " monitor owner:", await owner(), " bus:", await bus());
  // Create the scene from m1: Std → machine B / machine A, in both directions
  const mk = async (m, name, pickLabel) => { const p = await L.openUi(m, { hash: "#/monitors/scene" }); await L.sleep(1000); await p.locator("input").first().fill(name); await p.locator(".f", { hasText: "Std-34 shows" }).locator("[role=radio]").nth(pickLabel).click(); await p.getByRole("button", { name: "Save", exact: true }).click(); await L.sleep(1500); await p.ctx.close(); };
  const names = async (m) => [(await m.snapshot()).desk.machines.a.name, (await m.snapshot()).desk.machines.b.name];
  await mk(m1, "Std a A", 0); await mk(m1, "Std a B", 1);
  for (const [who, m] of [["m1", m1], ["m2", m2], ["m1", m1], ["m2", m2]]) {
    for (const sc of ["Std a A", "Std a B"]) {
      const p = await L.openUi(m, { hash: "#/monitors" }); await L.sleep(1500);
      const card = p.locator(`article[aria-label='Scene ${sc}']`); if (!(await card.getByRole("button", { name: "Apply" }).count())) { await p.ctx.close(); out.push(`${who} ${sc}: (ya active)`); continue; }
      await card.getByRole("button", { name: "Apply" }).click(); await L.sleep(4500);
      const t = (await L.text(p)).replace(/\n+/g, " | "); const ok = /active\./.test(t) && !/failed|not known/.test(t);
      out.push(`${who} applies «${sc}»: ${ok ? "OK" : "FAIL"} ${ok ? "" : t.match(/(not known[^|]*|the monitor said[^|]*|failed[^|]*)/)?.[0] ?? t.slice(120, 300)} bus=${JSON.stringify(await bus())}`);
      await p.ctx.close();
    }
  }
  console.log(out.join("\n")); await L.closeBrowser();
} catch (e) { console.log("ERROR", e, out); await L.closeBrowser(); }
