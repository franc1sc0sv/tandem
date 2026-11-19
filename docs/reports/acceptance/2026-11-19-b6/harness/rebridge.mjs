import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { readdirSync, readFileSync, writeFileSync } from "node:fs";
const tag = process.argv[2]; const ms = L.attach(tag);
for (const d of readdirSync("/proc").filter((x) => /^\d+$/.test(x))) { try { const cmd = readFileSync(`/proc/${d}/cmdline`, "utf8"); if (!cmd.includes("acc_bridge")) continue; const env = readFileSync(`/proc/${d}/environ`, "utf8"); if (env.includes(`/run/${tag}/`)) process.kill(+d, "SIGKILL"); } catch {} }
await L.sleep(500);
const info = JSON.parse(readFileSync(`${L.RUN}/${tag}/info.json`, "utf8"));
for (const w of ["m1", "m2"]) { await ms[w].startBridge(); info[w].bridgePid = ms[w].bridgePid; info[w].bridgePort = ms[w].bridgePort; }
writeFileSync(`${L.RUN}/${tag}/info.json`, JSON.stringify(info)); console.log("ok");
