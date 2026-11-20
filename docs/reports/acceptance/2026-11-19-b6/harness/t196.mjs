// T-196 on this box: eth1 holds 169.254.252.182 (a real link-local interface). Peers declared as "wifi" at that address
// must be classified by the local interface (Ethernet here), and an IPv6 link-local candidate with a scope must dial.
import * as L from "./lib.mjs"; import { execFileSync } from "node:child_process"; import { writeFileSync, readFileSync } from "node:fs";
const F = "T-196-link-classification-" + (process.argv[2] ?? "v4");
const { m1, m2 } = L.attach("host"); const mode = process.argv[2] ?? "v4";
try {
  for (const m of [m1, m2]) m.kill("SIGTERM");
  await L.sleep(2000);
  const addr = "169.254.252.182", v6 = "fe80::677:f28f:7726:2c3b%3";
  for (const [m, other] of [[m1, m2], [m2, m1]]) {
    let t = m.toml();
    t = t.replace(/peers = \[.*\]/, mode === "v4" ? `peers = [{ kind = "wifi", addr = "${addr}:${other.port}" }]` : `peers = [{ kind = "wifi", addr = "[${v6}]:${other.port}" }]`);
    writeFileSync(m.configFile, t);
    m.startAgent();
  }
  await L.sleep(9000);
  const out = execFileSync(L.bin("tandemd"), ["links", "--config", m1.configFile, "--data-dir", m1.dataDir], { env: { PATH: process.env.PATH, HOME: m1.root } }).toString();
  console.log(out);
  const s = await m1.snapshot();
  console.log("peer:", JSON.stringify(s.peer));
  const log = readFileSync(m1.log, "utf8").replace(/\x1b\[[0-9;]*m/g, "");
  const lines = log.split("\n").filter((l) => /healthy link|link down|active link|InvalidRemoteAddress|Connect\(/.test(l)).slice(-12).map((l) => l.slice(0, 260));
  console.log(lines.join("\n"));
  L.record(F, "peers declared 'wifi' at a link-local address reach paired state", s.peer.kind === "paired", JSON.stringify(s.peer));
  L.record(F, "no InvalidRemoteAddress for the IPv6 link-local candidate", !/InvalidRemoteAddress/.test(log), "");
  L.record(F, "the IPv4 link-local peer is classified by its interface (Ethernet), not as the declared wifi", /Ethernet/.test(JSON.stringify(s.peer)) || /Ethernet/.test(out.split("Paths")[1] ?? ""), JSON.stringify(s.peer));
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
