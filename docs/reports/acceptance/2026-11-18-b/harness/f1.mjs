import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "main");
const F = "F1-asistente";
const wiz = (p) => p.locator('[role=dialog][aria-label=Asistente]');
const step = (p, s) => p.locator("aside[aria-label=Pasos] button", { hasText: s }).click();
try {
  // m1: macOS-like (el puente dice que hay guía): se abre solo; m2: se abre desde Ajustes (Windows no la abre solo).
  const p1 = await L.openUi(m1);
  await L.sleep(1500);
  if ((await wiz(p1).count()) !== 1) { await p1.goto(m1.origin + "/#/ajustes"); await L.sleep(800); await p1.getByRole("button", { name: /Abrir/ }).first().click(); }
  await L.until("asistente m1 abierto", async () => (await wiz(p1).count()) === 1);
  const p2 = await L.openUi(m2, { hash: "#/ajustes" });
  await L.sleep(1200);
  const autoOpenM2 = (await wiz(p2).count()) === 1;
  L.record(F, "primer uso en máquina no macOS: el asistente se abre solo", autoOpenM2, autoOpenM2 ? "" : "no se abre solo; hay que ir a Ajustes → «Volver a correr el asistente»");
  if ((await wiz(p2).count()) !== 1) await p2.getByRole("button", { name: /Abrir/ }).first().click();
  await L.until("asistente m2 abierto", async () => (await wiz(p2).count()) === 1);
  for (const p of [p1, p2]) await step(p, "Tu puesto");
  await L.sleep(500);
  // m2 crea el puesto, m1 se une.
  await p2.getByRole("radio", { name: /Crear un puesto nuevo/ }).click();
  await L.until("código en m2", async () => (await wiz(p2).locator("#wz-new-code").count()) === 1, 20000);
  const code2 = (await wiz(p2).locator(".wz-code .wz-dg").allInnerTexts()).join("");
  await L.until("compare en m1", async () => (await wiz(p1).locator("#wz-code-1").count()) === 1, 20000);
  await L.shot(p2, F, "02-m2-codigo"); await L.shot(p1, F, "02-m1-escribir-codigo");
  const snap1 = await m1.snapshot(), snap2 = await m2.snapshot();
  const c1 = JSON.stringify(snap1.pairing), c2 = JSON.stringify(snap2.pairing);
  console.log("pairing m1:", c1); console.log("pairing m2:", c2);
  L.record(F, "las dos UIs muestran el mismo código de 6 dígitos", code2.length === 6 && c1.includes(code2) , `UI m2 muestra ${code2}; agente m1 ${c1.slice(0,120)}`);
  // Código equivocado en m1.
  await wiz(p1).locator("#wz-code-1").fill(code2 === "000000" ? "111111" : "000000");
  await L.sleep(500);
  const bad = await wiz(p1).locator("[role=alert]").count();
  L.record(F, "un código equivocado se rechaza con aviso", bad === 1);
  // Código correcto.
  await wiz(p1).locator("#wz-code-1").fill(code2);
  await L.until("emparejadas", async () => (await m1.snapshot()).peer.kind === "paired" && (await m2.snapshot()).peer.kind === "paired", 20000);
  await L.sleep(800);
  await L.shot(p1, F, "03-m1-emparejada"); await L.shot(p2, F, "03-m2-emparejada");
  const s1 = await m1.snapshot(), s2 = await m2.snapshot();
  L.record(F, "emparejadas: las dos agentes lo dicen y con lados distintos", s1.agent !== s2.agent, `m1=${s1.agent} m2=${s2.agent}`);
  console.log("peer m1:", JSON.stringify(s1.peer).slice(0, 300));
  console.log("trust m1:", require("node:fs").existsSync?.(m1.dataDir + "/trust.toml"));
  console.log("toml m1:\n" + m1.toml());
  console.log("toml m2:\n" + m2.toml());
  console.log(p1.errors, p2.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
