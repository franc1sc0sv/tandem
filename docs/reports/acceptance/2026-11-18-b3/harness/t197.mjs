// T-197: agents started with RELATIVE --data-dir/--config (cwd = machine root); workspace tunnel + bring_back must work.
import * as L from "./lib.mjs"; import { Mcp } from "./mcp.mjs";
import { spawn, execFileSync } from "node:child_process";
import { openSync, mkdirSync, writeFileSync, rmSync, readFileSync, existsSync, readdirSync } from "node:fs";
import { relative } from "node:path";
const F = "T-197-relative-data-dir";
const { m1, m2 } = L.attach("host");
const proj = "/tmp/claude-1000/acc3b/proj-rel"; rmSync(proj, { recursive: true, force: true }); mkdirSync(proj + "/src", { recursive: true });
const G = { GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t" };
const git = (cwd, ...a) => execFileSync("git", a, { cwd, env: { ...process.env, ...G } }).toString();
git(proj, "init", "-q", "-b", "main"); writeFileSync(proj + "/src/lib.rs", "fn a() {}\n"); git(proj, "add", "-A"); git(proj, "commit", "-q", "-m", "start");
try {
  for (const m of [m1, m2]) { m.kill("SIGTERM"); }
  await L.sleep(2000);
  const rels = [];
  for (const m of [m1, m2]) {
    const relData = relative(m.root, m.dataDir), relCfg = relative(m.root, m.configFile);
    rels.push(`${relData} | ${relCfg}`);
    const out = openSync(m.log, "a");
    const env = { PATH: process.env.PATH, HOME: m.root, TANDEM_LOG: "tandemd=debug" }; // no XDG/TANDEM_DATA_DIR: only the arguments
    const pr = spawn(L.bin("tandemd"), ["--sim-backends", "--data-dir", relData, "--config", relCfg], { cwd: m.root, env, stdio: ["ignore", out, out], detached: true });
    pr.unref(); m.pid = pr.pid;
  }
  console.log("relative args:", rels);
  await L.sleep(6000);
  const s = await m1.snapshot();
  L.record(F, "agents started with relative --data-dir/--config come up and stay paired", s.peer.kind === "paired", JSON.stringify(s.peer));
  const c = new Mcp(m1, proj); await c.init();
  const t = await c.call("tunnel_open", { workspace: true, title: "rel" });
  L.record(F, "tunnel_open with a workspace works", t.ok && !!t.value?.id, JSON.stringify(t.value ?? t.error).slice(0, 250));
  const id = t.value?.id; const ws = `${m2.dataDir}/workspaces/${id}`;
  await L.sleep(800);
  const files = existsSync(ws) ? readdirSync(ws, { recursive: true }).map(String) : [];
  L.record(F, "the project arrived on the other machine under an absolute path", files.includes("src/lib.rs"), files.filter((x) => !x.startsWith(".git/")).join(","));
  const e = await c.call("exec", { tunnel: id, cmd: "sh", args: ["-c", "echo new > src/new.rs; git add -A; git -c user.name=t -c user.email=t@t commit -qm from-b; echo done"] });
  await L.sleep(1500); const o = await c.call("output", { tunnel: id, process: e.value?.id ?? 0, wait_ms: 2000 });
  console.log("exec out:", JSON.stringify(o.value ?? o.error).slice(0, 200));
  const b = await c.call("bring_back", { tunnel: id, name: "rel" });
  L.record(F, "bring_back creates the branch in the origin project", b.ok && /tandem\/rel/.test(JSON.stringify(b.value)), JSON.stringify(b.value ?? b.error).slice(0, 250));
  console.log("branches:", git(proj, "branch", "--list").trim());
  const leftover = readdirSync(m1.dataDir + "/workspaces").filter((n) => /\.(in|out)$/.test(n));
  console.log("no stray relative dirs in cwd:", readdirSync(m1.root).join(","));
  c.close();
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
