import * as L from "./lib.mjs";
const tag = process.argv[2] ?? "host";
const opts = process.argv[3] ? JSON.parse(process.argv[3]) : {};
const { m1, m2 } = await L.startPair(tag, opts);
L.saveInfo(tag, m1, m2);
console.log(JSON.stringify({ m1: m1.origin, m2: m2.origin }));
