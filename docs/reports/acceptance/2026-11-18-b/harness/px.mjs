// Proxies UDP entre los dos agentes: un par por camino (usb4, wifi). Cortar un camino = crear el archivo cut-<camino>.
import dgram from "node:dgram"; import { existsSync, writeFileSync, readFileSync } from "node:fs"; import { join } from "node:path";
import * as L from "./lib.mjs";
const tag = process.argv[2]; const { m1, m2 } = L.attach(tag); const dir = join(L.RUN, tag);
const ports = {};
function proxy(kind, from, toPort) {
  return new Promise((res) => {
    const listen = dgram.createSocket("udp4"); const ups = new Map();
    listen.on("message", (msg, rinfo) => {
      if (existsSync(join(dir, `cut-${kind}`))) return;
      const key = `${rinfo.address}:${rinfo.port}`; let u = ups.get(key);
      if (!u) { u = dgram.createSocket("udp4"); u.bind(0, "127.0.0.1"); u.on("message", (m) => { if (!existsSync(join(dir, `cut-${kind}`))) listen.send(m, rinfo.port, rinfo.address); }); ups.set(key, u); }
      u.send(msg, toPort, "127.0.0.1");
    });
    listen.bind(0, "127.0.0.1", () => { ports[`${from}-${kind}`] = listen.address().port; res(); });
  });
}
for (const kind of ["usb4", "ethernet", "wifi"]) { await proxy(kind, "m1", m2.port); await proxy(kind, "m2", m1.port); }
writeFileSync(join(dir, "px.json"), JSON.stringify(ports)); console.log(JSON.stringify(ports));
setInterval(() => {}, 1000);
