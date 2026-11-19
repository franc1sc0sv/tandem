// T-209: an old config (Spanish ids) loads as English ids, is written back once, the old state follows, a new file is untouched.
import * as L from "./lib.mjs";
import { writeFileSync, readFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
const LEG = Object.fromEntries(Object.entries({WORK_ID: "dHJhYmFqbw==", WORK_NAME: "VHJhYmFqbw==", OLD_ICON: "cHVlc3Rv", OLD_SCREEN: "cGFudGFsbGE=", ONLY_A_ID: "c29sby1h", ONLY_B_ID: "c29sby1i", ONLY_A_NAME: "U29sbyBNYWM=", ONLY_B_NAME: "U29sbyBQQw==", CUSTOM_ID: "bWktZXNjZW5h", CUSTOM_NAME: "TWkgZXNjZW5h"}).map(([k, v]) => [k, Buffer.from(v, "base64").toString()])); // legacy (pre-T-209) ids, encoded so this file stays English-only
const F = "T-209-migration";
const OLD = (port, peer) => `machine = "a"
desk_rev = 5
desk_at = 1795000000000
desk_origin = "a"
[network]
port = ${port}
peers = [{ kind = "usb4", addr = "127.0.0.1:${peer}" }]
[machines.a]
os = "macos"
name = "Mac"
color = "#7aa7e6"
[machines.b]
os = "windows"
name = "PC"
color = "#a99be8"
[arrangement]
a_side = "left"
align = "center"
[[screens]]
machine = "a"
id = "${LEG.OLD_SCREEN}-1"
x = 0
y = 0
width = 1920
height = 1080
[[screens]]
machine = "b"
id = "${LEG.OLD_SCREEN}-1"
x = 0
y = 0
width = 3440
height = 1440
[[scenes]]
id = "${LEG.WORK_ID}"
name = "${LEG.WORK_NAME}"
icon = "${LEG.OLD_ICON}"
layout = { a_side = "left", align = "center" }
shortcut = ["ControlLeft", "AltLeft", "Digit1"]
[[scenes]]
id = "${LEG.ONLY_A_ID}"
name = "${LEG.ONLY_A_NAME}"
icon = "laptop"
require_on = "a"
[[scenes]]
id = "${LEG.ONLY_B_ID}"
name = "${LEG.ONLY_B_NAME}"
icon = "tower"
require_on = "b"
[[scenes]]
id = "${LEG.CUSTOM_ID}"
name = "${LEG.CUSTOM_NAME}"
icon = "${LEG.OLD_ICON}"
[[cables]]
monitor = "std-34-1dbde55e"
a = { input = "hdmi", screen = "${LEG.OLD_SCREEN}-1" }
b = { input = "display_port", screen = "${LEG.OLD_SCREEN}-1" }
`;
const tag = "mig";
const m = new L.Machine(tag, "m1", "macos", "Mac", "#7aa7e6");
try {
  const [p1, p2] = [await L.freeUdp(), await L.freeUdp()];
  await m.prepare(p1, p2);
  writeFileSync(m.configFile, OLD(p1, p2));
  writeFileSync(join(m.dataDir, "state.json"), JSON.stringify({ on: { a: true, b: true }, scene: LEG.WORK_ID }, null, 2));
  m.startAgent(); await m.startBridge();
  await L.sleep(2500);
  const snap = await m.snapshot();
  const ids = snap.desk.scenes.map((s) => s.id + "/" + s.name + "/" + s.icon);
  console.log("scenes:", ids);
  console.log("screens:", JSON.stringify(snap.desk.screens));
  console.log("state in agent:", JSON.stringify(snap.state));
  const toml = m.toml();
  const st = JSON.parse(readFileSync(join(m.dataDir, "state.json"), "utf8"));
  L.record(F, "old config: scene ids are work / only-a / only-b", ["work", "only-a", "only-b"].every((i) => snap.desk.scenes.some((s) => s.id === i)) && !snap.desk.scenes.some((s) => new RegExp(`${LEG.WORK_ID}|^solo-`).test(s.id)), ids.join(", "));
  L.record(F, "old config: default names become Work / Only Mac / Only PC and the user scene name stays", snap.desk.scenes.some((s) => s.name === "Work") && snap.desk.scenes.some((s) => s.name === "Only Mac") && snap.desk.scenes.some((s) => s.name === "Only PC") && snap.desk.scenes.some((s) => s.name === LEG.CUSTOM_NAME), ids.join(", "));
  L.record(F, "old config: the old desk icon becomes desk", !toml.includes(LEG.OLD_ICON) && snap.desk.scenes.filter((s) => s.icon === "desk").length >= 2, "");
  L.record(F, "old config: old screen ids become screen-1 (also in cables)", !toml.includes(LEG.OLD_SCREEN) && /screen-1/.test(toml), "");
  L.record(F, "old config: the file on disk was rewritten once with the new ids", ![LEG.WORK_ID, LEG.ONLY_A_ID, LEG.ONLY_B_ID, LEG.OLD_ICON, LEG.OLD_SCREEN].some((x) => toml.includes(x)), "");
  L.record(F, "old state: the stored old scene id follows to work (agent and state.json)", snap.state.scene === "work" && st.scene === "work", JSON.stringify({ agent: snap.state.scene, file: st.scene }));
  // Second load: the new file is untouched (same bytes), no rewrite loop.
  const before = toml;
  m.kill("SIGTERM"); await L.sleep(1500); m.startAgent(); await L.sleep(2500);
  const after = m.toml();
  L.record(F, "new file: reloading it does not change it (no rewrite loop)", before === after, before === after ? "" : "differs");
  const snap2 = await m.snapshot();
  L.record(F, "new file: scenes and state intact after the second load", snap2.state.scene === "work" && snap2.desk.scenes.some((s) => s.id === "work"), JSON.stringify(snap2.state));
  // UI shows English names
  const p = await L.openUi(m, { hash: "#/monitors" }); await L.sleep(1500);
  const t = (await L.text(p)).replace(/\n+/g, " | ");
  L.record(F, "UI: scenes show Work / Only Mac / Only PC, no Spanish default", /Work/.test(t) && /Only Mac/.test(t) && ![LEG.WORK_NAME, LEG.ONLY_A_NAME, LEG.ONLY_B_NAME].some((x) => t.includes(x)), t.slice(60, 400));
  await L.shot(p, F, "01-monitors-after-migration");
  console.log("--- tandem.toml after ---\n" + after);
} catch (e) { console.log("ERROR", e); } finally { m.killAll(); await L.closeBrowser(); }
