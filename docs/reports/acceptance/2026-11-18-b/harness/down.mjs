import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "main"); m1.killAll(); m2.killAll();
