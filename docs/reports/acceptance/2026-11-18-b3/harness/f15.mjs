import * as L from "./lib.mjs"; import { existsSync, readFileSync } from "node:fs";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F12-settings-palette";
try {
  const p = await L.openUi(m1, { hash: "#/settings" }); await L.sleep(1200);
  await p.getByRole("button", { name: "Leave", exact: true }).click(); await L.sleep(400);
  const first = (await p.locator("#pane").innerText()).includes("Confirm");
  await p.getByRole("button", { name: "Confirm" }).click(); const t0 = Date.now(); let tm2 = null; for (let i = 0; i < 40; i++) { if ((await m2.snapshot()).peer.kind === "unpaired") { tm2 = Date.now() - t0; break; } await L.sleep(500); } await L.sleep(500);
  const [s1, s2] = [await m1.snapshot(), await m2.snapshot()];
  const trust = [m1, m2].map((m) => existsSync(m.dataDir + "/trust.toml") ? readFileSync(m.dataDir + "/trust.toml", "utf8").trim().length : 0);
  L.record(F, "Settings → \"Leave desk\" (with confirmation): both machines forget the keys", first && s1.peer.kind === "unpaired" && s2.peer.kind === "unpaired" && trust.every((n) => n === 0), `peer m1=${s1.peer.kind} m2=${s2.peer.kind} (m2 took ${tm2 ?? "more than 20 s"} ms); trust.toml bytes=${trust}`);
  await L.shot(p, F, "05-leave-desk");
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); }
