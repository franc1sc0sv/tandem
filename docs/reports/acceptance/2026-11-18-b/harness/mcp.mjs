// Cliente MCP de prueba contra `tandem mcp` (JSON-RPC por stdio).
import { spawn } from "node:child_process";
import { bin } from "./lib.mjs";
export class Mcp {
  constructor(machine, cwd, extraEnv = {}) {
    this.p = spawn(bin("tandem"), ["mcp"], { cwd, env: { ...machine.env, ...extraEnv }, stdio: ["pipe", "pipe", "pipe"] });
    this.buf = ""; this.pending = new Map(); this.id = 0; this.stderr = "";
    this.p.stdout.on("data", (d) => { this.buf += d; let i; while ((i = this.buf.indexOf("\n")) >= 0) { const line = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1); if (!line.trim()) continue; const m = JSON.parse(line); const r = this.pending.get(m.id); if (r) { this.pending.delete(m.id); r(m); } } });
    this.p.stderr.on("data", (d) => (this.stderr += d));
  }
  rpc(method, params = {}) { const id = ++this.id; return new Promise((res) => { this.pending.set(id, res); this.p.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n"); }); }
  async init() { const r = await this.rpc("initialize", { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "acc-b", version: "1" } }); this.p.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n"); return r; }
  async tools() { return (await this.rpc("tools/list")).result.tools; }
  /** Devuelve {ok, value|error, raw}. */
  async call(name, args = {}) { const r = await this.rpc("tools/call", { name, arguments: args }); if (r.error) return { ok: false, error: r.error.message, raw: r }; const res = r.result; const txt = (res.content ?? []).map((c) => c.text).join("\n"); let value = res.structuredContent; if (!value) { try { value = JSON.parse(txt); } catch { value = txt; } } return { ok: !res.isError, value, error: res.isError ? txt : undefined, raw: r }; }
  close() { this.p.kill(); }
}
