// Round 6: repeated primary cuts on one pair. Each cut times how long until the agent reports input
// on another path (snapshot polled every 20 ms); then the cable comes back. Usage: node f6fail.mjs <tag> [N]
import * as L from "./lib.mjs"; import { writeFileSync, rmSync } from "node:fs";
const tag = process.argv[2]; const N = Number(process.argv[3] ?? 20); const { m1, m2 } = L.attach(tag);
const F = "F6-network-failover-loop";
const cut = (k) => writeFileSync(`${L.RUN}/${tag}/cut-${k}`, ""), heal = (k) => rmSync(`${L.RUN}/${tag}/cut-${k}`, { force: true });
const link = async (m) => (await m.snapshot()).peer.link;
const times = [];
for (let i = 0; i < N; i++) {
  await L.until("usb4 back", async () => (await link(m1)) === "usb4" && (await link(m2)) === "usb4", 30000, 200);
  await L.sleep(1500 + Math.random() * 1000); // random phase against the 1 s health ping
  const t = Date.now(); cut("usb4"); let took = null;
  while (Date.now() - t < 10000) { const l = await link(m1); if (l && l !== "usb4") { took = Date.now() - t; break; } await L.sleep(20); }
  times.push(took); console.log(i + 1, took);
  heal("usb4");
}
const over = times.filter((x) => x === null || x >= 1000);
L.record(F, `${tag}: ${N} primary cuts, input on the next path in < 1 s every time`, over.length === 0, `ms: ${times.join(", ")}; >= 1 s: ${over.length}/${N}; max ${Math.max(...times.map((x) => x ?? 1e9))}`);
