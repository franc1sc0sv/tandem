// Generates docs/reports/acceptance-2026-11-18-b.md from results.jsonl (latest run of each check).
import { readFileSync, writeFileSync } from "node:fs"; import { WT, RUN } from "./lib.mjs";
const rows = readFileSync(`${RUN}/results.jsonl`, "utf8").trim().split("\n").map((l) => JSON.parse(l));
const last = new Map(); for (const r of rows) last.set(r.flow + "|" + r.item, r); const items = [...last.values()];
const esc = (s) => String(s ?? "").replace(/\|/g, "¦").replace(/\n/g, " ").replace(/\s+/g, " ").trim();
const flows = [...new Set(items.map((r) => r.flow))];
const total = items.length, fails = items.filter((r) => !r.ok);
const SHOT = "acceptance/2026-11-19-b5";

let md = `# Tandem v0.3 acceptance — part B (end-to-end flows)

Date: 2026-11-18 · Base: \`origin/develop\` at \`04f2967\` (T-131) · Agent: acceptance (part B of 3). Out of scope: pixel-by-pixel parity (part A) and baseline CI quality (part C).

## Verdict

**FAIL.** ${total} checks run, ${total - fails.length} pass and ${fails.length} fail; the failures are grouped into 10 fix tasks (T-150 to T-159). A single failing item is enough not to declare the app finished.

## How it was tested

- Two real \`tandemd\` over loopback (real QUIC, orchestrator, IPC) with the production UI (\`pnpm build\`) in Chromium with Playwright. Since WebKitGTK is not in the environment, the Tauri glue was replaced by an **HTTP bridge** (\`apps/tandem-ui/src-tauri/examples/acc_bridge.rs\`) that uses the app's real code (\`session\`, \`shortcuts\`, \`setup\`, \`files\`, \`admin_service\`, \`update\`) and delivers events over SSE; the window sees a test \`__TAURI_INTERNALS__\`. Each machine has its own bridge and browser.
- Agents: (1) the real \`tandemd --sim-backends\` binary (pairing, saved arrangement, real tunnels through \`tandem mcp\`); (2) since \`--sim-backends\` has no input engine, monitors or clipboard, a **host** (\`crates/tandemd/examples/acc_host.rs\`, the same agent with \`daemon::start\`) with the repo's test doubles: input engine with fake backends, the fake DDC bus from \`monitor-ddc\` (standard and factory, with an injectable failed switch), fake clipboard and a declared operating system (\`--os mac|windows\`). A control port acts as the "physical world" (move the mouse, plug in the hub, add or remove monitors).
- Network with two and three paths: UDP proxies between the agents (usb4, ethernet, wifi) that can be cut and healed at will.
- The MCP client is a JSON-RPC script against \`tandem mcp\`.
- Real effect checked on the agent (state over IPC, \`tandem.toml\`, \`ui.toml\`, \`update.toml\`, the fake monitor's bus, \`sim-steps.jsonl\`, the workspace directory, git branches), not only on screen.
- Pairs tested: macOS+Windows (declared), Windows+Windows and macOS+macOS (G2). Demo mode: all routes and variants.
- Harness code: \`docs/reports/acceptance/2026-11-18-b/harness/\`. Screenshots: \`docs/reports/${SHOT}/<flow>/<step>.png\`.

### Environment limits (not app failures, but left unverified)

- WebKitGTK, the Tauri window, the tray icon and the system's global shortcuts do not exist here: the bridge records what the app would ask of the window (\`ui_show_main\`, \`ui_quit\`, \`files_open\`). The real updater (Tauri plugin) was replaced by a double with the same plan (\`update::plan\`/\`run\`) against a test \`latest.json\`; nothing is installed.
- Real macOS permissions, UAC, real Wake-on-LAN, real DDC, crossing with a real mouse and file drag with a real compositor: \`[hw]\`. File drag between machines was checked with its integration tests with doubles (\`cargo test -p tandemd --test drag --test files\`: 4 tests pass), not through the UI.
- "Data paused if input degrades" (F6): checked with \`cargo test -p tandem-sim --test network\` (4 tests pass, including \`measurement_is_cut_if_input_exceeds_its_target\`); the UI shows the "Wi-Fi only" case ("Data transfers wait for the cable").
- The hardware suite (T-123) was not run: H1–H5 are out of this part.

## Verdict per item of \`docs/ACCEPTANCE.md\`

| Item | Verdict | Tasks | Evidence |
|---|---|---|---|
`;
const V = [
  ["F1 First use (setup 1→6, pairing, permissions, keyboard and mouse, arrangement + crossing, summary, MCP)", "FAIL", "T-150, T-151, T-156, T-158", "Pairing with a 6-digit code and rejection of a bad code pass; missing: setup only on first use (Windows), step 4, crossing test, summary and \"Close\""],
  ["F2 Arrangement (4 sides × 3 alignments, engine)", "FAIL", "T-150", "All 12 cases are saved and the engine crosses through the new edge and not the old one; the UI does not draw the real screens and the crossing test cannot be completed"],
  ["F3 Scenes (create, edit, apply, injected failure, global shortcut)", "PASS", "", "F3-scenes"],
  ["F4 Monitors (standard with verdict, factory without writes, owner re-election)", "PASS", "", "F4-monitors · the real 34\" `[hw]` stays in the T-123 suite"],
  ["F5 Tunnels through MCP (git, 3 processes, output, files, port, link cut, bring_back, T-130)", "FAIL", "T-154, T-155", "Everything on the agent passes; a freshly opened UI does not show the existing tunnels and files_put of a credential gives the wrong error"],
  ["F6 Network (cut < 1 s, event in Network, measure bandwidth)", "PASS", "T-153 (related)", "F6-network · the measure button stays disabled after reloading on Network (T-153)"],
  ["F7 Keyboard and mouse without an owner", "PASS", "", "F7-no-owner"],
  ["F8 Clipboard and files", "PASS", "", "F8-clipboard · file drag through tests with doubles"],
  ["F9 Shared configuration and \"Undo\"", "PASS", "", "F9-shared-config"],
  ["F10 Power", "PASS", "T-159 (related)", "F10-power · with `[power.wol]` set by hand; without it \"Power on\" does not work (T-159)"],
  ["F11 Modules (power off/on, \"unavailable\" reasons)", "FAIL", "T-158", "Power off and on pass; the reasons for Tunnels and Files are false"],
  ["F12 Settings and palette (including \"Lock here\")", "FAIL", "T-152, T-157", "Almost all actions do what they say; \"Lock here\" locks but the UI never shows it and \"Leave desk\" leaves the other machine paired"],
  ["F13 Updates (test latest.json, plan for both)", "PASS", "", "F13-updates · real release documented in `docs/dev/release.md`"],
  ["F14 Tray", "PASS", "T-154 (related)", "F14-tray · the tray says \"0 tunnels\" with tunnels open (T-154)"],
  ["G2 Nothing fixed to Mac/PC (Windows+Windows, macOS+macOS)", "PASS", "", "G2-no-fixed-Mac-PC"],
  ["G6 Every interactive element does something real", "FAIL", "T-150, T-153", "Button sweep: \"Test crossing\" always disabled and \"Measure\" disabled after reload"],
  ["G7 / G8 Animations and reduced motion", "PASS", "", "G7-animations, G8-reduced-motion (sample of screens)"],
  ["G9 Console without errors / without `{{`, `undefined`, `NaN`", "PASS", "", "No console with errors during the whole live run and the demo-mode sweep"],
  ["G10 Everything editable is edited from the UI", "FAIL", "T-159", "Wake-on-LAN and the desk name cannot be edited from the UI"],
];
for (const [a, v, t, e] of V) md += `| ${a} | **${v}** | ${t} | ${esc(e)} |\n`;
md += `\nNot evaluated here: P1–P15 and similarity with the mockups (part A), G1/G3–G5/G11 baseline quality (part C) and H1–H5 (hardware).\n\n## Fix tasks

| Task | Failure | Flow |
|---|---|---|
| T-150 | Arrangement and crossing test without what the engine knows (real crossing, real screens) | F1, F2, G6 |
| T-151 | Setup step 4: detects what is assumed and does not hear the other machine | F1 |
| T-152 | "Leave desk" leaves the other machine paired | F12 |
| T-153 | Non-reactive \`can()\`: buttons disabled after reload | F6, G6 |
| T-154 | Empty tunnels in a freshly opened window | F5, F14 |
| T-155 | files_put of a credential answers "invalid manifest" | F5 |
| T-156 | Setup does not open by itself on Windows and "Close" is collapsed | F1 |
| T-157 | "Lock here" without a visible state | F12 |
| T-158 | Tunnels and Files modules with a false reason | F11, F1 |
| T-159 | Wake-on-LAN only through \`tandem.toml\`; desk name not editable | F10, G10 |\n\n`;
md += `## Reproduce

1. \`cargo build -p tandemd --features sim-backends -p tandem-cli\` and, with the sources from \`harness/\` copied to \`crates/tandemd/examples/acc_host.rs\` and \`apps/tandem-ui/src-tauri/examples/acc_bridge.rs\`, \`cargo build -p tandemd --features sim-backends --example acc_host\` and \`cargo build -p tandem-ui --example acc_bridge\`; \`pnpm -C apps/tandem-ui build\` and \`build:demo\`.
2. Copy the four binaries (\`tandemd\`, \`tandem\`, \`acc_host\`, \`acc_bridge\`) to \`/tmp/claude-1000/b5/bin\` (the path used by \`lib.mjs\`; change it in \`bin()\`).
3. \`node up.mjs <tag> '{"host":true}'\` brings up the pair; the \`f*.mjs\` scripts take the \`<tag>\`; \`all.sh\` chains the official run.

## Detail of each check\n\n`;
for (const f of flows) {
  md += `### ${f}\n\n| Check | Result | Detail |\n|---|---|---|\n`;
  for (const r of items.filter((x) => x.flow === f)) md += `| ${esc(r.item)} | ${r.ok ? "PASS" : "**FAIL**"} | ${esc(r.detail).slice(0, 260)} |\n`;
  md += `\nScreenshots: \`docs/reports/${SHOT}/${f}/\`\n\n`;
}
md += `## Other observations (no task)

- The "Changed from …" notices do not close by themselves (they ask for a decision) and cover controls in the top-right corner until dismissed; after "Undo", the notice that the other machine receives names the one that changed by its previous name.
- "Apply" on a scene whose monitors are not present says nothing (there is no notice that it did nothing).
- Scenes (F3): in 5 runs, 4 passed completely; in one the first application after adding the monitors failed with "it is not known which machine it shows (the real input or which input each machine uses is missing)" (the agent had not yet read the input or learned the cables), and in another the application went fine but the progress screen never showed (the transaction ended before opening it and went back to Monitors). They were not reduced to a single cause; if they reappear, they deserve their own task.
- The "link down" state resolved as "PC is off" when all three paths were cut for longer than \`pc_lost_after_secs\` (2 s in the test); with the default value it might look different: the range was not tested.
- "No agent" has no "Retry" button: it reconnects by itself every 2 s (\`agent_retry\` exists and the UI does not use it).
`;
writeFileSync(`${WT}/docs/reports/acceptance-2026-11-18-b.md`, md); console.log("written", total, "checks,", fails.length, "failures");
