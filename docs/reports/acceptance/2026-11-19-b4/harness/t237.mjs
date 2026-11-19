// T-237: one-way reachability. One agent cannot dial (its packets never reach the other side:
// ACC_NO_DIAL=1 in acc_host, i.e. NetOptions { dial: false }, or a peer address that drops everything).
// The pair is set up through the UI and then everything is checked from the side that cannot dial.
// Usage: node t237.mjs <tag> <m1|m2> [nodial|blackhole]
import * as L from "./lib.mjs"; import { Mcp } from "./mcp.mjs";
import { execFileSync } from "node:child_process"; import { mkdirSync, writeFileSync, rmSync, readFileSync } from "node:fs";
const tag = process.argv[2] ?? "ow"; const blocked = process.argv[3] ?? "m2"; const mode = process.argv[4] ?? "nodial";
const F = `T-237-one-way-${mode}-${blocked}`;
const { m1, m2 } = L.attach(tag);
const [nd, other] = blocked === "m1" ? [m1, m2] : [m2, m1];
async function entered(src, dst, [x, y, dx, dy], tries = 25) {
  for (let i = 0; i < tries; i++) {
    const before = (await dst.ctl({ cmd: "injected" })).count;
    await src.ctl({ cmd: "abs", x, y }); await src.ctl({ cmd: "rel", dx, dy }); await L.sleep(90);
    const r = await dst.ctl({ cmd: "injected" });
    if (r.count > before && r.tail.slice(0, r.count - before).some((e) => e.includes("MotionAbsolute"))) return true;
  }
  return false;
}
try {
  const s1 = await nd.snapshot(), s2 = await other.snapshot();
  const linkOf = (s) => JSON.stringify(s.peer?.link ?? s.peer).slice(0, 200);
  L.record(F, "the side that cannot dial sees the peer paired with a link", s1.peer.kind === "paired" && !!s1.peer.link, linkOf(s1));
  L.record(F, "the side that can dial sees the peer paired with a link", s2.peer.kind === "paired" && !!s2.peer.link, linkOf(s2));
  // Stability: 12 s of samples on both sides, the link must not change state.
  const seen = new Set(); let downs = 0;
  for (let i = 0; i < 24; i++) {
    for (const m of [nd, other]) { const s = await m.snapshot(); const k = s.peer.kind + ":" + (s.peer.link ? (s.peer.link.kind ?? "up") : "none"); seen.add(m.label + "=" + k); if (!s.peer.link) downs++; }
    await L.sleep(500);
  }
  L.record(F, "no flapping: 12 s of samples, the link never drops on either side", downs === 0, [...seen].join(" "));
  // UI of the side that cannot dial: Desk and Network show the link up.
  const p = await L.openUi(nd, { hash: "#/desk" }); await L.sleep(2000);
  const td = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "01-desk-no-dial-side");
  await p.goto(nd.origin + "/#/network"); await L.sleep(2000);
  const tn = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "02-network-no-dial-side");
  const trouble = /Link down|No link|not connected|Other machine off|Can't reach|cannot reach/i;
  L.record(F, "UI of the side that cannot dial: Desk and Network show the link up, no trouble state", !trouble.test(td) && !trouble.test(tn) && /USB-C|Ethernet|Wi-Fi/.test(tn), "desk: " + td.slice(0, 220) + " || network: " + tn.slice(0, 300));
  L.record(F, "no console errors on the side that cannot dial", p.errors.length === 0, p.errors.join(" ; ").slice(0, 300));
  await p.ctx.close();
  const pong = await nd.act({ kind: "ping_peer" }).catch((e) => ({ error: e.message }));
  L.record(F, "ping from the side that cannot dial is answered", pong.kind === "pong", JSON.stringify(pong).slice(0, 160));
  // Steps in both directions: rename from the side that cannot dial, then from the other side.
  const n1 = nd.name + "-x";
  await nd.act({ kind: "set_machine", machine: (await nd.snapshot()).agent, name: n1 }).catch((e) => console.log("rename", e.message));
  await L.until("rename reaches the other", async () => JSON.stringify((await other.snapshot()).desk.machines).includes(n1), 10000).then(() => true, () => false).then((ok) => L.record(F, "a change made on the side that cannot dial reaches the other machine", ok, n1));
  const n2 = other.name + "-y";
  await other.act({ kind: "set_machine", machine: (await other.snapshot()).agent, name: n2 }).catch((e) => console.log("rename", e.message));
  await L.until("rename reaches the no-dial side", async () => JSON.stringify((await nd.snapshot()).desk.machines).includes(n2), 10000).then(() => true, () => false).then((ok) => L.record(F, "a change made on the other side reaches the side that cannot dial", ok, n2));
  // Input: crossing in both directions.
  const g = await L.geoLeftCenter(m1, m2); await L.sleep(1000);
  const ab = await entered(m1, m2, g.m1m2); await L.sleep(1500);
  const ba = await entered(m2, m1, g.m2m1);
  L.record(F, "keyboard and mouse cross in both directions (engine crossing)", ab && ba, `m1->m2=${ab} m2->m1=${ba}`);
  // Tunnel through MCP from the side that cannot dial.
  const proj = `/tmp/claude-1000/acc4b/proj-ow-${tag}`; rmSync(proj, { recursive: true, force: true }); mkdirSync(proj, { recursive: true });
  const G = { ...process.env, GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t" };
  execFileSync("git", ["init", "-q", "-b", "main"], { cwd: proj }); writeFileSync(proj + "/a.txt", "a\n"); execFileSync("git", ["add", "-A"], { cwd: proj, env: G }); execFileSync("git", ["commit", "-q", "-m", "a"], { cwd: proj, env: G });
  const c = new Mcp(nd, proj); await c.init();
  const t = await c.call("tunnel_open", { workspace: true, title: "one-way" });
  let out = "";
  if (t.ok) {
    const e = await c.call("exec", { tunnel: t.value.id, cmd: "sh", args: ["-c", "echo one-way-ok; cat a.txt"] });
    await L.sleep(800); const o = await c.call("output", { tunnel: t.value.id, process: e.value?.id ?? 0, wait_ms: 3000 }); out = o.value?.stdout ?? JSON.stringify(o.error);
    await c.call("tunnel_close", { tunnel: t.value.id });
  }
  L.record(F, "a tunnel opened through MCP on the side that cannot dial runs a process on the other machine", t.ok && /one-way-ok/.test(out) && /^a$/m.test(out), JSON.stringify(t.value ?? t.error).slice(0, 120) + " out=" + out.slice(0, 80));
  c.close();
  const c2 = new Mcp(other, proj); await c2.init();
  const t2 = await c2.call("tunnel_open", { workspace: false, title: "reverse" }); let out2 = "";
  if (t2.ok) { const e = await c2.call("exec", { tunnel: t2.value.id, cmd: "echo", args: ["reverse-ok"] }); await L.sleep(800); const o = await c2.call("output", { tunnel: t2.value.id, process: e.value?.id ?? 0, wait_ms: 3000 }); out2 = o.value?.stdout ?? ""; await c2.call("tunnel_close", { tunnel: t2.value.id }); }
  L.record(F, "a tunnel from the other machine runs a process on the side that cannot dial", t2.ok && /reverse-ok/.test(out2), JSON.stringify(t2.value ?? t2.error).slice(0, 120) + " out=" + out2.slice(0, 80));
  c2.close();
  const log = nd.agentLog().replace(/\x1b\[[0-9;]*m/g, "");
  console.log("no-dial log tail:\n" + log.split("\n").filter((l) => /adopt|inbound|link|peer/i.test(l)).slice(-15).map((l) => l.slice(0, 240)).join("\n"));
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
