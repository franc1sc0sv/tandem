import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
const F = "F2-arrangement";
// m2 = A = PC 3440x1440; m1 = B = Mac 1920x1080. "Mac S" = Mac is at side S of the PC.
const geo = {
  right: { fromPc: [3439, 720, 8, 0], fromMac: [0, 540, -8, 0] },
  left: { fromPc: [0, 720, -8, 0], fromMac: [1919, 540, 8, 0] },
  up: { fromPc: [1720, 0, 0, -8], fromMac: [960, 1079, 0, 8] },
  down: { fromPc: [1720, 1439, 0, 8], fromMac: [960, 0, 0, -8] },
};
async function entered(src, dst, [x, y, dx, dy], tries) {
  for (let i = 0; i < tries; i++) {
    const before = (await dst.ctl({ cmd: "injected" })).count;
    await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90);
    const r = await dst.ctl({ cmd: "injected" });
    if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true;
  }
  return false;
}
const sideName = { up: "Arriba", down: "Abajo", left: "Izquierda", right: "Derecha" };
try {
  const p = await L.openUi(m1, { hash: "#/arrangement" }); await L.sleep(1000);
  let prev = null;
  for (const s of ["right", "up", "down", "left", "right"]) {
    await p.locator(`button.slot[aria-label="${sideName[s]}"]`).click();
    await p.locator("button.sb").nth(1).click();
    await p.getByRole("button", { name: "Guardar" }).click();
    await L.sleep(1500);
    const pos1 = await entered(m2, m1, geo[s].fromPc, 25), pos2 = await entered(m1, m2, geo[s].fromMac, 25);
    L.record(F, `engine: with Mac «${sideName[s]}» of the PC the crossing through the new edge works (PC→Mac and Mac→PC)`, pos1 && pos2, `PC→Mac ${pos1}, Mac→PC ${pos2}`);
    if (prev && prev !== s) {
      const neg = await entered(m2, m1, geo[prev].fromPc, 6);
      L.record(F, `engine: the old edge («${sideName[prev]}») no longer crosses`, !neg, neg ? "crossed through the old edge" : "");
    }
    prev = s;
  }
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
