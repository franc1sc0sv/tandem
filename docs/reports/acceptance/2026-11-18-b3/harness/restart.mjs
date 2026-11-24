import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs";
const [,, tag, who] = process.argv; const m = L.attach(tag)[who];
m.kill("SIGTERM"); await L.sleep(1500); m.startAgent();
const all = L.attach(tag); const info = JSON.parse(require("node:fs").readFileSync(`${L.RUN}/${tag}/info.json`, "utf8")); info[who].pid = m.pid; require("node:fs").writeFileSync(`${L.RUN}/${tag}/info.json`, JSON.stringify(info));
console.log("pid", m.pid);
