import { createRequire } from "node:module"; const require = createRequire(import.meta.url);
import * as L from "./lib.mjs"; import { readFileSync, writeFileSync } from "node:fs";
const tag = process.argv[2]; const ms = L.attach(tag); const px = JSON.parse(readFileSync(`${L.RUN}/${tag}/px.json`, "utf8"));
for (const who of ["m1", "m2"]) ms[who].kill("SIGTERM"); await L.sleep(2000);
for (const who of ["m1", "m2"]) {
  const m = ms[who]; let t = m.toml();
  t = t.replace(/peers = \[.*\]/, `peers = [{ kind = "usb4", addr = "127.0.0.1:${px[who + "-usb4"]}" }, { kind = "ethernet", addr = "127.0.0.1:${px[who + "-ethernet"]}" }, { kind = "wifi", addr = "127.0.0.1:${px[who + "-wifi"]}" }]`);
  writeFileSync(m.configFile, t); m.startAgent();
}
const info = JSON.parse(readFileSync(`${L.RUN}/${tag}/info.json`, "utf8")); for (const who of ["m1", "m2"]) info[who].pid = ms[who].pid; require("node:fs").writeFileSync(`${L.RUN}/${tag}/info.json`, JSON.stringify(info));
console.log("ok");
