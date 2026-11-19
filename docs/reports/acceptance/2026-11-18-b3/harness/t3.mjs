import * as L from "./lib.mjs";
const { m1, m2 } = L.attach(process.argv[2] ?? "host");
// The same standard monitor on both machines (the PC over DP, the Mac over HDMI); a factory one only on the PC;
// and a standard "Zfail-27" whose input change the firmware rejects (injected failure).
await m2.ctl({ cmd: "monitor_add", personality: "standard", name: "Std-34", serial: 1, input: "dp", via: "dp" });
await m1.ctl({ cmd: "monitor_add", personality: "standard", name: "Std-34", serial: 1, input: "dp", via: "hdmi" });
await m2.ctl({ cmd: "monitor_add", personality: "factory", name: "Factory-32", serial: 2 });
await m2.ctl({ cmd: "monitor_add", personality: "standard", name: "Zfail-27", serial: 3, input: "dp", via: "dp", switch: "fail" });
await m1.ctl({ cmd: "monitor_add", personality: "standard", name: "Zfail-27", serial: 3, input: "dp", via: "hdmi", switch: "fail" });
