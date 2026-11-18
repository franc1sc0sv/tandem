// Hallazgos deterministas contra el agente real (cada uno con su evidencia).
import * as L from "./lib.mjs"; import { readFileSync } from "node:fs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host3");
try {
  // F2/P6: la prueba de cruce en vivo
  { const F = "F2-prueba-de-cruce-en-vivo";
    const p = await L.openUi(m1, { hash: "#/puesto" }); await L.sleep(1200); await p.locator("nav a, aside a", { hasText: "Acomodo" }).first().click(); await L.sleep(1200);
    const dis = await p.getByRole("link", { name: /Probar cruce/ }).or(p.getByRole("button", { name: /Probar cruce/ })).first().getAttribute("aria-disabled").catch(() => null);
    const disabled = await p.locator("a:has-text('Probar cruce'), button:has-text('Probar cruce')").first().evaluate((e) => e.disabled === true || e.getAttribute("aria-disabled") === "true" || e.hasAttribute("disabled"));
    L.record(F, "Acomodo: «Probar cruce» está disponible con el agente real", !disabled, disabled ? "el botón está deshabilitado (store.can('crossing') siempre falso en LiveSource)" : "habilitado");
    await L.shot(p, F, "01-acomodo-probar-cruce-deshabilitado");
    await p.goto(m1.origin + "/#/acomodo/cruce"); await L.sleep(2500); const t = (await L.text(p)).replace(/\n+/g, " | ");
    L.record(F, "prueba de cruce con un CursorSample real del motor: se puede completar", /Cruzó bien/.test(t), t.slice(40, 300));
    await L.shot(p, F, "02-prueba-de-cruce-en-vivo");
    await p.ctx.close();
    // el motor sí informa el cursor: watch_cursor da muestras reales
    const r = await m1.act({ kind: "watch_cursor", on: true }); L.record(F, "el agente sí acepta watch_cursor (lo que falta es la UI)", r.kind === "done", JSON.stringify(r)); await m1.act({ kind: "watch_cursor", on: false });
  }
  // N4: can() no es reactivo: recargar en la pantalla deja botones deshabilitados
  { const F = "G6-botones-con-efecto";
    const p = await L.openUi(m1, { hash: "#/red" }); await L.sleep(3000);
    const dis = await p.getByRole("button", { name: /Medir ancho de banda/ }).isDisabled();
    L.record(F, "recargar la ventana en Red deja «Medir ancho de banda» habilitado", !dis, dis ? "queda deshabilitado para siempre (can() no es reactivo)" : "ok");
    await p.ctx.close(); }
  // Túneles al abrir
  { const F = "F5-tuneles-mcp"; const { Mcp } = await import("./mcp.mjs");
    const c = new Mcp(m1, "/tmp/claude-1000/accB"); await c.init(); const t = await c.call("tunnel_open", { title: "para-la-ui" });
    await L.sleep(1500); const p = await L.openUi(m1, { hash: "#/tuneles" }); await L.sleep(3000); const tx = (await L.text(p)).replace(/\n+/g, " | ");
    L.record(F, "la UI abierta (o recargada) después de abrir un túnel lo muestra sin esperar otro cambio", /para-la-ui/.test(tx), tx.slice(60, 220));
    await c.call("tunnel_close", { tunnel: t.value.id }); c.close(); await p.ctx.close(); }
  // Asistente: «Cerrar»
  { const F = "F1-asistente"; const p = await L.openUi(m1, { hash: "#/ajustes" }); await L.sleep(1000); await p.getByRole("button", { name: /Abrir/ }).first().click(); await L.sleep(1500);
    const b = await p.locator(".wz-close").boundingBox(); const hit = await p.evaluate(() => { const e = document.querySelector(".wz-close").getBoundingClientRect(); return document.elementFromPoint(e.x + e.width / 2, e.y + e.height / 2)?.className; });
    L.record(F, "el botón «Cerrar» del asistente es visible y se puede pulsar", b.height >= 24 && /wz-close/.test(String(hit)), `caja ${Math.round(b.width)}×${Math.round(b.height)} px; lo que recibe el clic: ${hit}`);
    await L.shot(p, F, "00-cerrar-colapsado"); await p.ctx.close(); }
  // Máquina: reglas «no disponible»
  { const F = "F1-asistente"; const p = await L.openUi(m1, { hash: "#/ajustes" }); await L.sleep(800); await p.getByRole("button", { name: /Abrir/ }).first().click(); await L.sleep(800); await p.keyboard.press("Escape");
    await p.locator("body").click({ position: { x: 5, y: 5 } }).catch(() => {}); await p.evaluate(() => { location.hash = "#/puesto"; });
    await p.goto(m1.origin + "/#/ajustes"); await p.getByRole("button", { name: /Abrir/ }).first().click(); await L.sleep(500); await p.locator("aside[aria-label=Pasos] button", { hasText: "Listo" }).click(); await L.sleep(800);
    const t = (await p.locator('[role=dialog][aria-label=Asistente] main').innerText()).replace(/\n+/g, " | ");
    L.record(F, "resumen final: «Cómputo remoto» refleja que los túneles funcionan", !/falta el cómputo remoto/.test(t), t.slice(0, 340)); await L.shot(p, F, "06-resumen-computo"); await p.ctx.close(); }
  // Paso 1 del asistente: Archivos y Cómputo
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
