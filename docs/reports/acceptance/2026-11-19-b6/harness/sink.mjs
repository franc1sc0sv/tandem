// A UDP address that silently drops everything (a firewall in stealth mode). Usage: node sink.mjs <port>
import dgram from "node:dgram";
const s = dgram.createSocket("udp4"); let n = 0;
s.on("message", () => { n++; }); s.bind(Number(process.argv[2]), "127.0.0.1");
setInterval(() => console.log("dropped", n), 5000);
