import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F2-acomodo";
const W = { m1: [1920, 1080], m2: [3440, 1440] };
async function entered(src, dst, x, y, dx, dy, tries = 12) {
  for (let i = 0; i < tries; i++) {
    const before = (await dst.ctl({ cmd: "injected" })).count;
    await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90);
    const r = await dst.ctl({ cmd: "injected" });
    if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true;
  }
  return false;
}
const sideName = { up: "Arriba", down: "Abajo", left: "Izquierda", right: "Derecha" };
const side = async (m) => (await m.snapshot()).agent;
try {
  const sa = (await side(m1)) === "a" ? m1 : m2, sb = sa === m1 ? m2 : m1; // sa = máquina A
  const nameA = sa === m1 ? "m1" : "m2", nameB = sb === m1 ? "m1" : "m2";
  console.log("A =", nameA, " B =", nameB);
  const p = await L.openUi(m1, { hash: "#/acomodo" });
  await L.sleep(1200);
  const slotLabel = (await p.locator("text=/¿Dónde está/").first().innerText());
  console.log(slotLabel);
  const opp = { up: "down", down: "up", left: "right", right: "left" };
  const xName = slotLabel.match(/¿Dónde está (.*)\?/)[1];
  const xIsA = (sa === m1 ? "Mac" : "PC") === xName; // m1 = Mac, m2 = PC
  console.log("seleccionada:", xName, "es A:", xIsA);
  for (const s of ["right", "up", "down", "left"]) {
    for (const [ai, align] of [[0, "start"], [1, "center"], [2, "end"]]) {
      await p.locator(`button.slot[aria-label="${sideName[s]}"]`).click();
      await p.locator("button.sb").nth(ai).click();
      await p.getByRole("button", { name: "Guardar" }).click();
      await L.sleep(900);
      const [d1, d2] = [(await m1.snapshot()).desk.arrangement, (await m2.snapshot()).desk.arrangement];
      const wantSide = xIsA ? s : opp[s];
      const ok = [d1, d2].every((a) => a && a.a_side === wantSide && a.align === align);
      const tomls = [m1, m2].map((m) => m.toml()).map((t) => (t.match(/\[arrangement\][^\[]*/) ?? [""])[0].replace(/\n/g, " "));
      L.record(F, `${xName} «${sideName[s]}», alineación ${align}: llega a las dos máquinas y a tandem.toml`, ok && tomls.every((t) => t.includes(`a_side = "${wantSide}"`) && t.includes(`align = "${align}"`)), ok ? "" : JSON.stringify({ d1, d2, tomls: tomls[0] }));
    }
    await L.shot(p, F, `lado-${s}`);
  }
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
