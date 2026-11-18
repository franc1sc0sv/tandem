import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "win2"); const F = "F2-acomodo";
try {
  const p = await L.openUi(m1, { hash: "#/acomodo" }); await L.sleep(1500);
  const t = (await L.text(p)).replace(/\n+/g, " | ");
  console.log(t.slice(0, 900));
  const boxes = await p.locator("button.screen, [data-screen], .sc").evaluateAll((els) => els.map((e) => { const r = e.getBoundingClientRect(); return [e.className, Math.round(r.width), Math.round(r.height), e.getAttribute("aria-label")]; })).catch(() => []);
  console.log(JSON.stringify(boxes));
  const real = /1920/.test(t) && /3440/.test(t) && /1080/.test(t) && /1440/.test(t);
  L.record(F, "Acomodo muestra resolución real de las dos pantallas (1920×1080 y 3440×1440)", real, t.slice(0, 400));
  await L.shot(p, F, "20-acomodo-real");
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
