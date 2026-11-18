import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "main");
const F = "F1-setup";
const wiz = (p) => p.locator('[role=dialog][aria-label=Setup]');
const step = (p, s) => p.locator("aside[aria-label=Pasos] button", { hasText: s }).click();
try {
  // m1: macOS-like (the bridge says there is a tour): opens by itself; m2: opened from Settings (Windows does not open it by itself).
  const p1 = await L.openUi(m1);
  await L.sleep(1500);
  if ((await wiz(p1).count()) !== 1) { await p1.goto(m1.origin + "/#/settings"); await L.sleep(800); await p1.getByRole("button", { name: /Open/ }).first().click(); }
  await L.until("setup m1 open", async () => (await wiz(p1).count()) === 1);
  const p2 = await L.openUi(m2, { hash: "#/settings" });
  await L.sleep(1200);
  const autoOpenM2 = (await wiz(p2).count()) === 1;
  L.record(F, "first run on a non-macOS machine: setup opens by itself", autoOpenM2, autoOpenM2 ? "" : "does not open by itself; you have to go to Settings → \"Run setup again\"");
  if ((await wiz(p2).count()) !== 1) await p2.getByRole("button", { name: /Open/ }).first().click();
  await L.until("setup m2 open", async () => (await wiz(p2).count()) === 1);
  for (const p of [p1, p2]) await step(p, "Your desk");
  await L.sleep(500);
  // m2 creates the desk, m1 joins.
  await p2.getByRole("radio", { name: /Create a new desk/ }).click();
  await L.until("code on m2", async () => (await wiz(p2).locator("#wz-new-code").count()) === 1, 20000);
  const code2 = (await wiz(p2).locator(".wz-code .wz-dg").allInnerTexts()).join("");
  await L.until("compare on m1", async () => (await wiz(p1).locator("#wz-code-1").count()) === 1, 20000);
  await L.shot(p2, F, "02-m2-code"); await L.shot(p1, F, "02-m1-type-code");
  const snap1 = await m1.snapshot(), snap2 = await m2.snapshot();
  const c1 = JSON.stringify(snap1.pairing), c2 = JSON.stringify(snap2.pairing);
  console.log("pairing m1:", c1); console.log("pairing m2:", c2);
  L.record(F, "both UIs show the same 6-digit code", code2.length === 6 && c1.includes(code2) , `UI m2 shows ${code2}; agent m1 ${c1.slice(0,120)}`);
  // Wrong code on m1.
  await wiz(p1).locator("#wz-code-1").fill(code2 === "000000" ? "111111" : "000000");
  await L.sleep(500);
  const bad = await wiz(p1).locator("[role=alert]").count();
  L.record(F, "a wrong code is rejected with a notice", bad === 1);
  // Correct code.
  await wiz(p1).locator("#wz-code-1").fill(code2);
  await L.until("paired", async () => (await m1.snapshot()).peer.kind === "paired" && (await m2.snapshot()).peer.kind === "paired", 20000);
  await L.sleep(800);
  await L.shot(p1, F, "03-m1-paired"); await L.shot(p2, F, "03-m2-paired");
  const s1 = await m1.snapshot(), s2 = await m2.snapshot();
  L.record(F, "paired: both agents say so, with different sides", s1.agent !== s2.agent, `m1=${s1.agent} m2=${s2.agent}`);
  console.log("peer m1:", JSON.stringify(s1.peer).slice(0, 300));
  console.log("trust m1:", require("node:fs").existsSync?.(m1.dataDir + "/trust.toml"));
  console.log("toml m1:\n" + m1.toml());
  console.log("toml m2:\n" + m2.toml());
  console.log(p1.errors, p2.errors);
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
