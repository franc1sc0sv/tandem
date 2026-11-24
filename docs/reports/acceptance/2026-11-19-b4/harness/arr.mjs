import * as L from "./lib.mjs";
const { m1, m2 } = L.attach("host");
try {
  for (const m of [m1, m2]) {
    const p = await L.openUi(m, { hash: "#/arrangement" }); await L.sleep(1500);
    const t = (await L.text(p)).replace(/\n+/g, " | ");
    console.log(m.label, t.slice(60, 700));
    L.record("P5-arrangement", `${m.label}: resolution, ratio, scale shown`, /1920 × 1080/.test(t) || /1920/.test(t), "");
    await L.shot(p, "P5-arrangement", `arrangement-${m.label}`);
    console.log(p.errors);
  }
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
