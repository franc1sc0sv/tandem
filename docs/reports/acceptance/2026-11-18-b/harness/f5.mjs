import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { Mcp } from "./mcp.mjs";
import { execFileSync } from "node:child_process"; import { mkdirSync, writeFileSync, rmSync, readFileSync, existsSync, readdirSync } from "node:fs"; import net from "node:net";
const tag = process.argv[2] ?? "host3"; const { m1, m2 } = L.attach(tag);
const F = "F5-tuneles-mcp";
const proj = "/tmp/claude-1000/accB/proj-demo"; rmSync(proj, { recursive: true, force: true }); mkdirSync(proj + "/src", { recursive: true }); mkdirSync(proj + "/.aws", { recursive: true });
const G = { GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t" };
const git = (cwd, ...a) => execFileSync("git", a, { cwd, env: { ...process.env, ...G } }).toString();
git(proj, "init", "-q", "-b", "main"); writeFileSync(proj + "/src/lib.rs", "fn a() {}\n"); git(proj, "add", "-A"); git(proj, "commit", "-q", "-m", "inicio");
git(proj, "remote", "add", "origin", "https://github.com/ejemplo/ejemplo.git"); git(proj, "config", "credential.helper", "store --file=/tmp/secret-cred");
writeFileSync(proj + "/src/nuevo.rs", "fn n() {}\n"); writeFileSync(proj + "/.env", "TOKEN=supersecreto\n"); writeFileSync(proj + "/deploy.pem", "-----BEGIN PRIVATE KEY-----\n"); writeFileSync(proj + "/id_rsa", "-----BEGIN RSA PRIVATE KEY-----\n"); writeFileSync(proj + "/.npmrc", "//registry.npmjs.org/:_authToken=npm_abcdef\n"); writeFileSync(proj + "/.aws/credentials", "[default]\naws_secret_access_key=xx\n");
const c = new Mcp(m1, proj, { SSH_AUTH_SOCK: "/tmp/ssh-agent.sock", GH_TOKEN: "ghp_origen" }); await c.init();
const ok = (r) => r.ok;
try {
  // 0. herramientas (P15)
  const names = (await c.tools()).map((t) => t.name);
  const want = ["machines", "resources", "tunnels", "tunnel_open", "tunnel_close", "exec", "processes", "output", "kill", "files_put", "files_get", "port_forward", "port_stop", "bring_back"];
  L.record(F, "P15: las herramientas de Claude existen en `tandem mcp`", want.every((w) => names.includes(w)), names.join(","));
  // 1. abrir túnel con workspace
  const t = await c.call("tunnel_open", { workspace: true, title: "ws-aceptacion" });
  L.record(F, "tunnel_open con workspace de git", ok(t) && t.value.id, JSON.stringify(t.value ?? t.error).slice(0, 300));
  const id = t.value.id; const ws = `${m2.dataDir}/workspaces/${id}`;
  await L.sleep(500);
  const files = existsSync(ws) ? readdirSync(ws, { recursive: true }).map(String) : [];
  const leaked = ["\.env", "deploy.pem", "id_rsa", ".npmrc", ".aws"].filter((f) => files.some((x) => x === f || x.startsWith(f) || x.endsWith("/" + f)));
  L.record(F, "T-130: la otra máquina no recibe credenciales (.env, .pem, id_rsa, .npmrc, .aws)", leaked.length === 0 && files.includes("src/lib.rs") && files.some((x) => x.endsWith("nuevo.rs")), `archivos en destino: ${files.filter((x) => !x.startsWith(".git/")).join(",")} | filtrados: ${JSON.stringify(t.value.skipped)} | aviso: ${t.value.warning ? "sí" : "no"}`);
  const gcfg = existsSync(ws + "/.git/config") ? readFileSync(ws + "/.git/config", "utf8") : "(sin .git/config)";
  L.record(F, "T-130: el repo llega sin remotos ni credential.helper", !/remote|credential|ejemplo/.test(gcfg), gcfg.replace(/\n/g, " ").slice(0, 200));
  // 2. exec rechaza secretos en env
  const bad = await c.call("exec", { tunnel: id, cmd: "env", env: ["GH_TOKEN=x"] });
  L.record(F, "T-130: exec rechaza variables de entorno que parecen secretos", !bad.ok, JSON.stringify(bad.error ?? bad.value).slice(0, 200));
  const allowed = await c.call("exec", { tunnel: id, cmd: "sh", args: ["-c", "echo GH=$GH_TOKEN SSH=$SSH_AUTH_SOCK AWS=$AWS_SECRET_ACCESS_KEY"], env: ["AWS_SECRET_ACCESS_KEY=yy"], allow_secret_env: true });
  await L.sleep(800); const ao = await c.call("output", { tunnel: id, process: allowed.value?.id ?? 0, wait_ms: 2000 });
  L.record(F, "T-130: con allow_secret_env pasa solo lo pedido; GH_TOKEN y SSH_AUTH_SOCK del origen nunca", allowed.ok && /AWS=yy/.test(ao.value?.stdout ?? "") && !/ghp_origen|\/tmp\/ssh-agent\.sock/.test(ao.value?.stdout ?? ""), JSON.stringify(ao.value?.stdout ?? ao.error ?? allowed.error));
  // 3. tres procesos a la vez
  const procs = [];
  for (const n of [1, 2, 3]) procs.push((await c.call("exec", { tunnel: id, cmd: "sh", args: ["-c", `for i in 1 2 3 4 5 6; do echo p${n}-$i; sleep 1; done`] })).value.id);
  await L.sleep(1500);
  const pl = await c.call("processes", { tunnel: id });
  const running = (pl.value.processes ?? pl.value).filter((x) => x.state === "Running" || x.state === "running");
  L.record(F, "3 procesos corriendo a la vez en el túnel", running.length >= 3, `corriendo: ${running.length} de ${JSON.stringify((pl.value.processes ?? pl.value).map((x) => x.state))}`);
  // UI en vivo mientras corren
  const p = await L.openUi(m1, { hash: "#/tuneles" }); await L.sleep(2000);
  const ttInit = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "la UI recién abierta muestra los túneles que ya existen (sin esperar a un cambio)", /ws-aceptacion/.test(ttInit), ttInit.slice(80, 260));
  await c.call("exec", { tunnel: id, cmd: "true" }); await L.sleep(2500);
  const tt = (await L.text(p)).replace(/\n+/g, " | ");
  await L.shot(p, F, "01-ui-tres-procesos");
  const p2 = await L.openUi(m2, { hash: "#/tuneles" }); await L.sleep(1500); await c.call("exec", { tunnel: id, cmd: "true" }); await L.sleep(1500); await c.call("exec", { tunnel: id, cmd: "true" }); await L.sleep(2000);
  const tt2 = (await L.text(p2)).replace(/\n+/g, " | ");
  L.record(F, "la UI (origen) muestra el proyecto, el túnel y sus procesos en vivo", /proj-demo/.test(tt) && /ws-aceptacion/.test(tt), tt.slice(80, 500));
  L.record(F, "la UI de la otra máquina también muestra el túnel", /ws-aceptacion/.test(tt2), tt2.slice(80, 400));
  // outputs
  const outs = [];
  for (const [i, pid] of procs.entries()) { const o = await c.call("output", { tunnel: id, process: pid, wait_ms: 8000 }); outs.push(o.value); }
  L.record(F, "leer la salida de cada proceso, sin mezclarse", outs.every((o, i) => o.stdout.includes(`p${i + 1}-1`) && !o.stdout.includes(`p${((i + 1) % 3) + 1}-1`)), JSON.stringify(outs.map((o) => o.stdout.replace(/\n/g, ",").slice(0, 60))));
  // 4. archivos
  writeFileSync("/tmp/claude-1000/accB/entrega.txt", "contenido-de-ida\n");
  const put = await c.call("files_put", { tunnel: id, local: "/tmp/claude-1000/accB/entrega.txt", remote: ws + "/entrega.txt" });
  const putBad = await c.call("files_put", { tunnel: id, local: proj + "/id_rsa", remote: ws + "/id_rsa" });
  await L.sleep(500); const there = existsSync(ws + "/entrega.txt") ? readFileSync(ws + "/entrega.txt", "utf8") : null;
  L.record(F, "copiar un archivo al túnel (files_put)", put.ok && there === "contenido-de-ida\n", JSON.stringify(put.value ?? put.error).slice(0, 200));
  L.record(F, "T-130: files_put de una credencial se omite con aviso", !existsSync(ws + "/id_rsa") && /omiti|credencial|skipped/i.test(JSON.stringify(putBad.value ?? putBad.error)), JSON.stringify(putBad.value ?? putBad.error).slice(0, 250));
  writeFileSync(ws + "/salida.txt", "contenido-de-vuelta\n");
  const get = await c.call("files_get", { tunnel: id, remote: ws + "/salida.txt", local: "/tmp/claude-1000/accB/salida-local.txt" });
  L.record(F, "traer un archivo del túnel (files_get)", get.ok && existsSync("/tmp/claude-1000/accB/salida-local.txt") && readFileSync("/tmp/claude-1000/accB/salida-local.txt", "utf8") === "contenido-de-vuelta\n", JSON.stringify(get.value ?? get.error).slice(0, 200));
  // 5. puerto
  const rp = 41000 + Math.floor(Math.random() * 1000), lp = rp + 1000;
  const srv = await c.call("exec", { tunnel: id, cmd: "python3", args: ["-c", `import socket;s=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);s.bind(("127.0.0.1",${rp}));s.listen(1)\nwhile True:\n  c,_=s.accept();d=c.recv(100);c.sendall(b"eco:"+d);c.close()`] });
  await L.sleep(1000);
  const pf = await c.call("port_forward", { tunnel: id, local_port: lp, remote_port: rp });
  await L.sleep(500);
  const echo = await new Promise((res) => { const s = net.connect(lp, "127.0.0.1"); let b = ""; s.on("data", (d) => (b += d)); s.on("close", () => res(b)); s.on("error", (e) => res("ERR " + e.message)); s.write("hola"); setTimeout(() => { s.destroy(); res(b || "timeout"); }, 4000); });
  L.record(F, "abrir un puerto (port_forward) y hablar por él", pf.ok && echo === "eco:hola", `${JSON.stringify(pf.value ?? pf.error).slice(0, 150)} → ${echo}`);
  if (pf.ok && pf.value.id !== undefined) await c.call("port_stop", { id: pf.value.id });
  // 6. cortar el enlace y recuperarlo sin perder salida
  const lg = await c.call("exec", { tunnel: id, cmd: "sh", args: ["-c", "for i in $(seq 1 24); do echo n$i; sleep 0.5; done"] });
  await L.sleep(2000); const first = await c.call("output", { tunnel: id, process: lg.value.id, wait_ms: 500 });
  const { writeFileSync: w } = await import("node:fs");
  for (const k of ["usb4", "wifi"]) w(`${L.RUN}/${tag}/cut-${k}`, "");
  await L.sleep(5000);
  const during = (await c.call("output", { tunnel: id, process: lg.value.id, wait_ms: 500 }));
  for (const k of ["usb4", "wifi"]) rmSync(`${L.RUN}/${tag}/cut-${k}`, { force: true });
  await L.sleep(9000);
  let acc = first.value.stdout, cur = first.value.next; let tries = 0, last;
  while (tries++ < 10) { const o = await c.call("output", { tunnel: id, process: lg.value.id, from_stdout: cur.stdout, from_stderr: cur.stderr, wait_ms: 4000 }); if (!o.ok) { last = o.error; await L.sleep(1500); continue; } acc += o.value.stdout; cur = o.value.next; if (o.value.done) break; }
  const got = acc.trim().split("\n"); const seqOk = got.length === 24 && got.every((l, i) => l === `n${i + 1}`);
  L.record(F, "cortar el enlace y recuperarlo sin perder salida (24 líneas seguidas)", seqOk, `líneas ${got.length}${seqOk ? "" : ": " + got.join(",").slice(0, 160)}; durante el corte: ${during.ok ? "respondió" : "error: " + String(during.error).slice(0, 100)}${last ? " | " + last : ""}`);
  // 7. bring_back
  await c.call("exec", { tunnel: id, cmd: "sh", args: ["-c", "echo 'fn desde_b() {}' > src/b.rs && git add -A && git -c user.email=t@t -c user.name=t commit -qm desde-b"] }); await L.sleep(1500);
  const bb = await c.call("bring_back", { tunnel: id, name: "ret" });
  const branches = git(proj, "branch", "--list", "tandem/*");
  L.record(F, "bring_back crea la rama tandem/<nombre> en el proyecto", bb.ok && /tandem\/ret/.test(branches), `${JSON.stringify(bb.value ?? bb.error).slice(0, 200)} | ramas: ${branches.trim()}`);
  if (/tandem\/ret/.test(branches)) { const l = git(proj, "show", "tandem/ret:src/b.rs"); L.record(F, "bring_back: la rama trae el cambio hecho en la otra máquina", /desde_b/.test(l)); }
  // 8. UI de solo lectura
  await p.goto(m1.origin + "/#/tuneles"); await L.sleep(1800);
  const btns = await p.locator("main button, main input, main textarea").evaluateAll((els) => els.filter((e) => e.offsetParent).map((e) => ((e.getAttribute("aria-label") || e.innerText || e.placeholder || e.tagName) + "").trim().replace(/\n/g, " ")));
  console.log("controles en Túneles:", btns);
  await L.shot(p, F, "02-ui-despues");
  console.log("UI errores:", p.errors, p2.errors);
  await L.closeBrowser();
} catch (e) { console.log("ERROR", e); await L.closeBrowser(); } finally { c.close(); }
