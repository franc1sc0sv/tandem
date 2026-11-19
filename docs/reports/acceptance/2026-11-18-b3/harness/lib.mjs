// Acceptance harness B: two real tandemd (--sim-backends) over loopback, one HTTP bridge per machine
// (apps/tandem-ui/src-tauri/examples/acc_bridge.rs) and the production UI in Chromium with Playwright.
import { spawn, execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { mkdirSync, rmSync, writeFileSync, readFileSync, existsSync, appendFileSync } from "node:fs";
import { createServer } from "node:net";
import dgram from "node:dgram";
import { join } from "node:path";

export const WT = process.env.WT ?? new URL("../../../../..", import.meta.url).pathname;
export const TARGET = process.env.CARGO_TARGET_DIR ?? `${process.env.HOME}/.cache/tandem-target-acc3b`;
export const RUN = process.env.ACC_RUN ?? "/tmp/claude-1000/acc3b/run";
export const SHOTS = process.env.ACC_SHOTS ?? join(WT, "docs/reports/acceptance/2026-11-18-b");
const require = createRequire(join(WT, "apps/tandem-ui/package.json"));
export const { chromium } = require("playwright");

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const bin = (n) => (n.startsWith("examples/acc_") ? join("/tmp/claude-1000/acc3b/bin", n.slice(9)) : join("/tmp/claude-1000/acc3b/bin", n));

export function freeUdp() {
  return new Promise((res) => {
    const s = dgram.createSocket("udp4");
    s.bind(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => res(p)); });
  });
}
export function freeTcp() {
  return new Promise((res) => {
    const s = createServer();
    s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => res(p)); });
  });
}

export const results = [];
export function record(flow, item, ok, detail = "") {
  results.push({ flow, item, ok, detail });
  try { appendFileSync(join(RUN, "results.jsonl"), JSON.stringify({ flow, item, ok, detail }) + "\n"); } catch {}
  console.log(`${ok ? "PASA " : "FALLA"} [${flow}] ${item}${detail ? " — " + detail : ""}`);
}

/** One machine: agent + bridge. `os` is what its tandem.toml declares. */
export class Machine {
  constructor(tag, label, os, name, color) {
    this.tag = tag; this.label = label; this.os = os; this.name = name; this.color = color;
    this.root = join(RUN, tag, label);
    this.cfg = join(this.root, "cfg");
    this.data = join(this.root, "data");
    this.env = { ...process.env, XDG_CONFIG_HOME: this.cfg, XDG_DATA_HOME: this.data, HOME: this.root, TANDEM_DATA_DIR: join(this.data, "Tandem") };
    this.configFile = join(this.cfg, "Tandem", "tandem.toml");
    this.dataDir = join(this.data, "Tandem");
  }
  async prepare(port, peerPort, extra = "", side = "a") {
    rmSync(this.root, { recursive: true, force: true });
    mkdirSync(join(this.cfg, "Tandem"), { recursive: true });
    mkdirSync(this.dataDir, { recursive: true, mode: 0o700 });
    this.port = port;
    writeFileSync(this.configFile,
`machine = "${side}"
[network]
port = ${port}
peers = [{ kind = "usb4", addr = "127.0.0.1:${peerPort}" }]
[orchestrator]
power_on_timeout_secs = 5
step_timeout_secs = 3
[power]
pc_lost_after_secs = 2
[machines.${side}]
os = "${this.os}"
name = "${this.name}"
color = "${this.color}"
${extra}`);
  }
  startAgent() {
    this.log = join(this.root, "agent.out");
    const out = require("node:fs").openSync(this.log, "a");
    const [cmd, args] = this.host ? [bin("examples/acc_host"), ["--os", this.os === "macos" ? "mac" : "windows", "--control", String(this.ctlPort), "--screen", this.screen ?? "1920x1080"]] : [bin("tandemd"), ["--sim-backends"]];
    this.proc = spawn(cmd, args, { env: { ...this.env, TANDEM_LOG: "tandemd=debug,tandem_link=debug" }, stdio: ["ignore", out, out], detached: true });
    this.proc.unref(); this.pid = this.proc.pid;
    return this.proc;
  }
  /** Control of the "physical world" (host only): one JSON line, one response. */
  async ctl(obj) {
    const { connect } = await import("node:net");
    return new Promise((res, rej) => { const s = connect(this.ctlPort, "127.0.0.1"); let b = ""; s.on("data", (d) => { b += d; if (b.includes("\n")) { s.end(); res(JSON.parse(b)); } }); s.on("error", rej); s.write(JSON.stringify(obj) + "\n"); });
  }
  async startBridge(extraArgs = []) {
    this.bridgePort = await freeTcp();
    this.blog = join(this.root, "bridge.out");
    const out = require("node:fs").openSync(this.blog, "a");
    const env = this.os === "macos" ? { ...this.env, ACC_MAC_SETUP: "1" } : this.env;
    this.bridge = spawn(bin("examples/acc_bridge"), ["--port", String(this.bridgePort), "--static", join(WT, "apps/tandem-ui/dist"), ...extraArgs], { env, stdio: ["ignore", out, out], detached: true });
    this.bridge.unref(); this.bridgePid = this.bridge.pid;
    for (let i = 0; i < 100; i++) { try { const r = await fetch(`${this.origin}/__bridge/calls`); if (r.ok) return; } catch {} await sleep(100); }
    throw new Error("the bridge did not start: " + readFileSync(this.blog, "utf8"));
  }
  get origin() { return `http://127.0.0.1:${this.bridgePort}`; }
  async calls() { return (await fetch(`${this.origin}/__bridge/calls`)).json(); }
  async invoke(cmd, args = {}) {
    const r = await fetch(`${this.origin}/__bridge/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args) });
    const j = await r.json(); if (!r.ok) throw new Error(j.error); return j;
  }
  act(action) { return this.invoke("agent_act", { action }); }
  snapshot() { return this.invoke("agent_snapshot"); }
  kill(sig = "SIGTERM") { try { process.kill(this.pid, sig); } catch {} }
  killAll() { for (const p of [this.pid, this.bridgePid]) { try { process.kill(p, "SIGKILL"); } catch {} } }
  info() { return { tag: this.tag, label: this.label, os: this.os, name: this.name, color: this.color, port: this.port, pid: this.pid, host: this.host, ctlPort: this.ctlPort, screen: this.screen, bridgePid: this.bridgePid, bridgePort: this.bridgePort }; }
  static fromInfo(i) { const m = new Machine(i.tag, i.label, i.os, i.name, i.color); Object.assign(m, { host: i.host, ctlPort: i.ctlPort, screen: i.screen, port: i.port, pid: i.pid, bridgePid: i.bridgePid, bridgePort: i.bridgePort, log: join(m.root, "agent.out"), blog: join(m.root, "bridge.out") }); return m; }
  async restartAgent() { this.startAgent(); }
  agentLog() { try { return readFileSync(this.log, "utf8"); } catch { return ""; } }
  toml() { return readFileSync(this.configFile, "utf8"); }
}

export async function startPair(tag, { a = ["macos", "Mac", "#7aa7e6"], b = ["windows", "PC", "#a99be8"], extra = "", host = false, screens = ["1920x1080", "3440x1440"] } = {}) {
  const m1 = new Machine(tag, "m1", ...a), m2 = new Machine(tag, "m2", ...b);
  const [p1, p2] = [await freeUdp(), await freeUdp()];
  const hubCfg = host ? '\n[input.hub]\nkeyboard = "046d:c33f"\nmouse = "046d:c539"\n' : "";
  await m1.prepare(p1, p2, extra + hubCfg); await m2.prepare(p2, p1, extra + hubCfg);
  if (host) { m1.host = m2.host = true; m1.ctlPort = await freeTcp(); m2.ctlPort = await freeTcp(); [m1.screen, m2.screen] = screens; }
  m1.startAgent(); m2.startAgent();
  for (let i = 0; i < 100; i++) { if (existsSync(join(m1.dataDir, "agent.sock")) || true) break; }
  await m1.startBridge(); await m2.startBridge();
  return { m1, m2 };
}

export const INIT = `(() => {
  const handlers = new Map(); let n = 0, eid = 0;
  window.__TAURI_INTERNALS__ = {
    transformCallback(cb, once) { const id = ++n; window['_' + id] = (p) => { if (once) delete window['_' + id]; return cb && cb(p); }; return id; },
    unregisterCallback(id) { delete window['_' + id]; },
    convertFileSrc: (x) => x,
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main', windowLabel: 'main' } },
    invoke: async (cmd, args) => {
      if (cmd === 'plugin:event|listen') { const id = ++eid; const l = handlers.get(args.event) ?? []; l.push({ id, cbId: args.handler }); handlers.set(args.event, l); return id; }
      if (cmd === 'plugin:event|unlisten') { for (const [k, l] of handlers) handlers.set(k, l.filter((x) => x.id !== args.eventId)); return; }
      const r = await fetch('/__bridge/invoke/' + cmd, { method: 'POST', body: JSON.stringify(args || {}) });
      const j = await r.json(); if (!r.ok) throw j.error; return j;
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  const es = new EventSource('/__bridge/events');
  es.onmessage = (e) => { const { event, payload } = JSON.parse(e.data); for (const l of handlers.get(event) ?? []) { const f = window['_' + l.cbId]; if (f) f({ event, id: l.id, payload }); } };
})();`;

let browser;
export async function getBrowser() {
  browser ??= await chromium.launch({ args: ["--no-sandbox"] });
  return browser;
}
export async function closeBrowser() { await browser?.close(); browser = undefined; }

/** Opens a machine's UI (real agent through the bridge) or demo mode (own `url`). */
export async function openUi(machine, { hash = "", query = "", viewport = { width: 1280, height: 860 }, reducedMotion = "no-preference" } = {}) {
  const ctx = await (await getBrowser()).newContext({ viewport, locale: "en-US", colorScheme: "dark", reducedMotion });
  await ctx.addInitScript(INIT);
  const page = await ctx.newPage();
  const errors = [];
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  await page.goto(`${machine.origin}/${query}${hash}`);
  page.errors = errors; page.ctx = ctx;
  return page;
}

export async function openDemo(origin, query, hash = "", viewport = { width: 1280, height: 860 }, reducedMotion = "no-preference") {
  const ctx = await (await getBrowser()).newContext({ viewport, locale: "en-US", colorScheme: "dark", reducedMotion });
  const page = await ctx.newPage();
  const errors = [];
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  await page.goto(`${origin}/${query}${hash}`);
  page.errors = errors; page.ctx = ctx;
  return page;
}

export async function shot(page, flow, step) {
  const dir = join(SHOTS, flow); mkdirSync(dir, { recursive: true });
  const f = join(dir, `${step}.png`);
  await page.screenshot({ path: f });
  return f;
}

export async function until(what, fn, ms = 15000, every = 100) {
  const t = Date.now(); let last;
  while (Date.now() - t < ms) { try { const v = await fn(); if (v) return v; } catch (e) { last = e; } await sleep(every); }
  throw new Error(`time timed-out: ${what}${last ? " (" + last.message + ")" : ""}`);
}

export const text = (page) => page.evaluate(() => document.body.innerText);

export function saveInfo(tag, m1, m2) { writeFileSync(join(RUN, tag, "info.json"), JSON.stringify({ m1: m1.info(), m2: m2.info() })); }
export function attach(tag) { const i = JSON.parse(readFileSync(join(RUN, tag, "info.json"), "utf8")); return { m1: Machine.fromInfo(i.m1), m2: Machine.fromInfo(i.m2) }; }

/** Leaves the arrangement as "A to the left of B, centered" and returns the crossing geometry between m1 and m2 according to their sides. */
export async function geoLeftCenter(m1, m2) {
  await m1.act({ kind: "set_arrangement", a_side: "left", align: "center" }); await sleep(1500);
  const a1 = (await m1.snapshot()).agent;
  const [w1, h1] = m1.screen.split("x").map(Number), [w2, h2] = m2.screen.split("x").map(Number);
  return a1 === "a"
    ? { m1m2: [w1 - 1, h1 / 2, 8, 0], m2m1: [0, h2 / 2, -8, 0] }
    : { m1m2: [0, h1 / 2, -8, 0], m2m1: [w2 - 1, h2 / 2, 8, 0] };
}
