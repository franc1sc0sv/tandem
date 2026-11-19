// T-280: on a pair where the non-owner lost the owner's report, apply a scene from the non-owner's
// window and capture the result. Usage: node t280b.mjs <tag>
import * as L from "./lib.mjs";
const tag = process.argv[2]; const { m1, m2 } = L.attach(tag); const F = "T-280-report-lost-on-owner-change";
const s1 = await m1.snapshot(); const std = s1.monitors.find((m) => m.name === "Std-34");
const [owner, other] = std.owner === s1.agent ? [m1, m2] : [m2, m1];
const so = await other.snapshot(); const v = so.monitors.find((m) => m.name === "Std-34");
console.log("owner", std.owner, "non-owner report", JSON.stringify(v.report), "owner report", JSON.stringify((await owner.snapshot()).monitors.find((m) => m.name === "Std-34").report?.input));
if (v.report) { console.log("NOT REPRODUCED"); process.exit(0); }
const p = await L.openUi(other, { hash: "#/monitors" }); await L.sleep(2000);
await L.shot(p, F, "01-non-owner-monitors");
// Std-34 starts on DP, which is m2's input (t3.mjs): show m1.
const d = (await other.snapshot()).desk; const m1side = (await m1.snapshot()).agent;
const card = p.locator(`article[aria-label='Scene Only ${d.machines[m1side].name}']`);
const sceneName = await card.getAttribute("aria-label");
await card.getByRole("button", { name: "Apply" }).click(); await L.sleep(4000);
const t = (await L.text(p)).replace(/\n+/g, " | ");
await L.shot(p, F, "02-apply-from-non-owner");
const bus = (await owner.ctl({ cmd: "monitor_screen", id: 0 })).input;
console.log(sceneName, "| bus", bus, "|", t.slice(0, 600));
L.record(F, "a scene applied from the machine that does not own the monitor switches it", !/not known which machine is shown/.test(t), `${sceneName}; bus=${bus}; ${t.slice(0, 400)}`);
await L.closeBrowser();
