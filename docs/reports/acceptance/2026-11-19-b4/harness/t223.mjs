// T-223: two configured peers that fall on the same local interface (eth2 here: an IPv4 address and
// its IPv6 link-local with the scope id). Before the fix the link manager refused to start
// ("the link ... is configured twice") and the agent had no link at all.
// Usage: node t223.mjs <tag>  (a paired pair; both agents are restarted with the new peers)
import * as L from "./lib.mjs"; import { execFileSync } from "node:child_process"; import { writeFileSync } from "node:fs";
const tag = process.argv[2] ?? "tw"; const F = "T-223-two-peers-one-interface";
const { m1, m2 } = L.attach(tag);
const ip = (process.env.ACC_V4 ?? "192.168.1.172"), v6 = (process.env.ACC_V6 ?? "fe80::e2e1:5573:27b0:2173%5");
try {
  for (const m of [m1, m2]) m.kill("SIGTERM");
  await L.sleep(2500);
  for (const [m, other] of [[m1, m2], [m2, m1]]) {
    const t = m.toml().replace(/peers = \[.*\]/, `peers = [{ kind = "wifi", addr = "${ip}:${other.port}" }, { kind = "ethernet", addr = "[${v6}]:${other.port}" }]`);
    writeFileSync(m.configFile, t);
    m.startAgent();
  }
  await L.until("paired with a link", async () => { const a = await m1.snapshot(), b = await m2.snapshot(); return a.peer.link && b.peer.link; }, 30000).catch(() => {});
  await L.sleep(3000);
  const out = execFileSync(L.bin("tandemd"), ["links", "--config", m1.configFile, "--data-dir", m1.dataDir], { env: { PATH: process.env.PATH, HOME: m1.root } }).toString();
  console.log(out);
  const s1 = await m1.snapshot(), s2 = await m2.snapshot();
  const log = m1.agentLog().replace(/\x1b\[[0-9;]*m/g, "");
  const lines = log.split("\n").filter((l) => /configured twice|only the first one|link manager|healthy link|active link/i.test(l)).slice(-10).map((l) => l.slice(0, 260));
  console.log(lines.join("\n"));
  L.record(F, "the link manager starts (no 'configured twice')", !/configured twice/.test(log), lines.filter((l) => /twice/.test(l)).join(" ").slice(0, 200));
  L.record(F, "both agents are paired with a working link", s1.peer.kind === "paired" && !!s1.peer.link && !!s2.peer.link, JSON.stringify(s1.peer.link) + " / " + JSON.stringify(s2.peer.link));
  L.record(F, "a warning in the log names the candidate that was left out", /only the first one is used/.test(log), lines.filter((l) => /only the first one/.test(l)).join(" ").slice(0, 300));
  const pong = await m1.act({ kind: "ping_peer" }).catch((e) => ({ error: e.message }));
  L.record(F, "ping answers over the kept path", pong.kind === "pong", JSON.stringify(pong).slice(0, 160));
  const p = await L.openUi(m1, { hash: "#/network" }); await L.sleep(2500);
  const tn = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "01-network");
  L.record(F, "Network shows a notice for the left-out candidate and the link up", /also reach(es)? the .* path; only .* is used/.test(tn) && !/Link down/i.test(tn), tn.slice(0, 600));
  L.record(F, "no console errors", p.errors.length === 0, p.errors.join(";").slice(0, 200));
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
