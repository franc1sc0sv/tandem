import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2]??"host"); await m1.invoke("setup_finish"); await m2.invoke("setup_finish"); console.log(JSON.stringify(await m1.invoke("setup_get")));
