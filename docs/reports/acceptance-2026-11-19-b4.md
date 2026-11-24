# Tandem v0.3 acceptance - part B, round 4 (end-to-end flows)

Date: 2026-11-19 - Base: `origin/develop` at `8504769` (T-240). `develop` later gained `bf38f34`, `3f12397`, `92a1dc1` and `fcdaf1e`, which only touch docs, CI and the hardware scripts (no agent or UI code), so the run stands for them too. Protocol v41. Agent: acceptance (part B). Out of scope: pixel parity (part A) and baseline CI quality (part C).

## Verdict

**FAIL 2.** Every round 3 failure is fixed (T-220, T-221, T-222, T-223), and everything new since round 3 works end to end: one-way reachability (T-237) in two variants, two peers on one interface (T-223), the key-probe replay in setup step 4 (T-240), synthetic motion refused without the test config (T-239), the honest "Administrator service" row on Linux (T-027), and desk_sync and drag (T-236, T-230/T-235). Two new failures, both UI races between a command reply and the notices that follow it:

- **T-250**: "Save and apply" lands on Monitors instead of Changes in progress in 5 of 8 runs (F3, P9).
- **T-251**: during "Update all" the Updates pane never shows the step and "Update all" stays enabled (F13, P11).

## How it was tested

Same harness as rounds 1-3, adapted (`docs/reports/acceptance/2026-11-19-b4/harness/`). Two real `tandemd` over loopback. The host (`acc_host.rs`) runs doubles for the input engine, DDC and clipboard. One HTTP bridge per machine (`acc_bridge.rs`) runs the app core's real code. The production UI runs in Chromium with Playwright; demo mode runs on its own bridge. UDP proxies cut paths, and an MCP client runs against `tandem mcp`. New pieces for this round:

- `acc_host.rs` reads `ACC_NO_DIAL=1` and then starts with `NetOptions { dial: false }`. It plays a machine whose packets never reach the other side, like the T-237 test hook.
- `sink.mjs` is a UDP address that silently drops everything (a stealth firewall), used as the configured peer address of one side.
- `acc_motion.rs` is a small IPC client (example of `tandem-hw`) that sends `UiCommand::SyntheticMotion`.
- `lib.mjs` gained per-machine environment and peer lists. New scripts: `t237.mjs`, `t223.mjs`, `t240.mjs`, `admin.mjs`, `f3prog.mjs`, `f3prog2.mjs`, `upd2.mjs`, `upd3.mjs`, `xtimeout.mjs`, `xtimeout2.mjs`, `curprobe.mjs`, `sink.mjs`.

Pairs: macOS+Windows (`fin`, `r2`, plus `ow` and `bh` for T-237), Windows+Windows (`win2`) and macOS+macOS (`mac2`). Demo mode ran with every route and variant. Raw results are in `results.jsonl`. It holds the whole session, including the harness false negatives listed under observations. Screenshots are in `docs/reports/acceptance/2026-11-19-b4/<flow>/`. The Tunnels ones were removed, as in round 3, because they show the test project's path.

Environment limits (unchanged): no WebKitGTK, no system tray or global shortcuts, the real updater replaced by a double, and everything `[hw]` left for the T-123 suite or each task's pending hardware list.

## Round 3 failures, checked again

| Task | What was checked | Result |
|---|---|---|
| T-220 | Both windows on step 4 (`t166.mjs`, `t166b.mjs`, macOS+macOS pair): a key on A, then a key on B. Both windows end with **both** rows marked "Activity detected"; the baseline shows nothing before pressing; the reverse order too. | PASS |
| T-221 | Settings -> Updates shows the switch "Update automatically" and it is saved in `update.toml` (`auto = false`). Network says "Primary"/"Backup", no "Main" or "Band". | PASS |
| T-222 | Every field name in all agent logs of the session (`id command link addr from txn ... enabled kept dropped`): no Spanish word, no accented character. | PASS |
| T-223 | Both agents restarted with `peers = [{ kind = "wifi", addr = "192.168.1.172:<port>" }, { kind = "ethernet", addr = "[fe80::...%5]:<port>" }]`, both on the box's real `eth2` (`t223.mjs`). The link manager starts (no "configured twice"). Both agents are paired over Ethernet, and ping answers in 1 ms. The log says `two peers reach the same link: only the first one is used kind=Ethernet kept=[fe80::...%5]:<port> dropped=192.168.1.172:<port>`. Network shows the notice "192.168.1.172:<port> also reaches the Ethernet path; only [fe80::...%5]:<port> is used. Remove the extra peer..." with the link up (`T-223-two-peers-one-interface/01-network.png`). | PASS |

## What changed since round 3

| Task | What was checked | Result |
|---|---|---|
| T-237 (no dial) | Pair `ow`: B starts with `ACC_NO_DIAL=1` and **creates** the desk; A joins with the code through the UI. Pairing works. B logs `using the connection the other agent opened link=USB4`. From B, the side that cannot dial (`t237.mjs ow m2`): both agents paired with a link. 12 s of samples on both sides show no drop (no flapping). B's Desk and Network show the link up with no trouble state and no console errors (`T-237-one-way-nodial-m2/01-desk-no-dial-side.png`, `02-network-no-dial-side.png`). Ping from B answers (`pong usb4 1 ms`). A rename on B reaches A and a rename on A reaches B. Keyboard and mouse cross in both directions (engine crossing). A tunnel opened through MCP on B runs a process on A over a git workspace, and a tunnel from A runs on B. | PASS |
| T-237 (stealth firewall) | Pair `bh`: after pairing, A's configured peer address is replaced by a UDP sink that drops everything and A is restarted. A has the link 2 s later, and the sink saw only 2 packets, so A adopted B's connection instead of dialling. Every check above passes again from A (`t237.mjs bh m1 blackhole`, `T-237-one-way-blackhole-m1/`). | PASS |
| T-240 | Pair `ow` (B cannot dial, so its own link only appears when it adopts A's connection). Both windows are on step 4, then B is restarted. The key is reported on B 526 ms after the restart, while **B's own link is not installed yet** (checked: A sees the link, B does not). A's window marks B's row "Activity detected" once B's link appears (`t240.mjs`, `T-240-key-probe-replay/01-other-window.png`). | PASS |
| T-239 | `acc_motion` against an agent with a normal configuration: `Rejected("synthetic motion is off on this agent ([input] synthetic_motion)")`. Against an agent whose `tandem.toml` has `[input] synthetic_motion = true`: `Done`, twice. `tandem mcp` exposes no motion tool (the F5 tool list). | PASS |
| T-027 | Setup step 1 "This machine" on both machines (one declared macOS, one Windows; the app core runs on Linux): the row "Administrator service" says "the administrator service only exists on macOS and Windows", with no Install button. `admin_status` is `{state: off, canInstall: false}`, `admin_install` is refused with the same reason, nothing is elevated and there are no console errors (`admin.mjs`, `T-027-admin-service-row/`). | PASS |
| T-236 | F9 "a simultaneous change converges" passes. `desk_sync_never_mixes_a_stamp_with_another_desk` passes. | PASS |
| T-230, T-235 | `cargo test -p tandemd --test drag`: 4 passed, including the forcing test for a `Begin` that arrives before `serve`. The file-copy notice with progress and "Copied" works end to end (`f16b.mjs`). The real drag across the edge stays `[hw]` (the host has no file-drag backend, so Files reports "This system has no file drag between machines", which is correct). | PASS |

## Verdict per item of `docs/ACCEPTANCE.md`

| Item | Verdict | Tasks | Evidence |
|---|---|---|---|
| F1 First use | PASS | | Setup opens by itself; the same 6-digit code shows on both UIs; a wrong code is rejected; paired with different sides (also with one side unable to dial). Keyboard detected with and without the hub. Step 4 marks both machines in both windows (T-220, T-240). Step 5: a real engine crossing gives "Crossed to PC through the right edge, as the arrangement says" and enables Continue (`wiz5.mjs`). The summary says "from any machine · crossing tested" and "MCP ready for Claude Code on both", and the MCP command is copied. Permissions are doubles (`[hw]` real) |
| F2 Arrangement | PASS | | 4 sides x 3 alignments reach both machines and `tandem.toml`. The engine crosses through the new edge and not through the old one. The Arrangement crossing test gives "Crossed correctly · It entered PC" with a real engine crossing, and "Did not cross in 20 s" at 20 s (3 fresh windows, plus 2 runs after a crossing, plus with the cursor left on the other machine) |
| F3 Scenes | **FAIL** | T-250 | Create, edit, apply (the monitor switches, "Active" on both UIs), injected failure with visible rollback and the desk restored, global scene shortcut: all pass. But "Save and apply" skips Changes in progress in 5 of 8 runs |
| F4 Monitors | PASS | | Factory monitor shows "Unsupported" with zero DDC writes. The input change shows the monitor's verdict. Owner re-election works when the owner powers off. `[hw]` 34" stays in T-123 |
| F5 Tunnels through MCP | PASS | | All 18 checks: tools, git workspace, credentials filtered, secret env refused, 3 processes at once, output, files in and out, port forward, cut and recover without losing output, `bring_back`. A freshly opened UI shows the tunnel. Also from the side that cannot dial (T-237) |
| F6 Network | PASS | | Primary cut moves input to the next path in 614 ms; the event is visible; the primary takes input back; "Measure bandwidth" gives a value |
| F7 Keyboard and mouse without an owner | PASS | | The one in use is in charge; no stuck keys on an owner change or a link drop |
| F8 Clipboard and files | PASS | | Text A->B and B->A, an image A->B. The file copy notice shows progress. Drag is covered by the CI doubles (`drag` tests); real drag is `[hw]` |
| F9 Shared configuration | PASS | | Rename, color, lock shortcut, Cmd<->Ctrl, clipboard switch reach the other machine. "Changed from ... Undo" restores. A simultaneous rename converges |
| F10 Power | PASS | | Power off and on (simulated Wake-on-LAN) with steps recorded by both agents. Lost machine -> "PC is off · Power on PC" |
| F11 Modules | PASS | | The other machine's clipboard and keyboard-and-mouse switches work from its page; "unavailable" reasons are correct |
| F12 Settings and palette | PASS | | Every D15 palette command and its navigation; Ctrl+, ; "Open setup"; "Change the other machine..."; "Measure the link"; "Lock here" locks the engine and the Desk shows "Cursor locked here"; "Remove the lock"; "Lock here" on the machine not in control explains why; window shortcut saved in `ui.toml`; "Leave desk" with confirmation unpairs both (trust files end with `peer = []`) |
| F13 Updates | **FAIL** | T-251 | Version lookup against a test `latest.json`. "Update all" asks the other machine first (its agent logs the request), and after 20 s without it the UI says "the other machine did not come back". 404 -> "could not check". "Update automatically" is saved. But during the run the pane never shows the step and "Update all" stays enabled. Trouble states (permissions, link down, other machine off, no agent) work with their actions |
| F14 Tray | PASS | | State, a scene applied from the tray reaches the agent, "Open" and "Quit" reach the app core. Windows+Windows and macOS+macOS: "1 tunnel" per machine, nothing names the other system |
| G2 Nothing fixed to Mac/PC | PASS | | Windows+Windows and macOS+macOS pairs: no screen names the other system or uses its shortcuts |
| G6 Every element does something real | PASS | | Button sweep on every live screen. The only "no effect" result is the input radio a monitor is already on ("PC DP" of Zfail-27, `aria-checked=true`), which is the current input |
| G7 / G8 Animations and reduced motion | PASS | | Demo: 39 animated elements on Desk; nothing animated with `prefers-reduced-motion: reduce` (part A covers parity) |
| G9 Language and console | PASS | | 37 live pages (every screen, Settings tab and wizard step) and the demo sweep (all pair variants, routes and states): no Spanish, `{{`, `undefined` or `NaN`, and no console errors. Agent logs are in English. The one console error seen comes from pressing "Update all" twice during a run (T-251) |
| G10 Everything editable is edited from the UI | PASS | | Wake-on-LAN MAC and the desk name reach both `tandem.toml` |

Not evaluated here: P1-P15 pixel similarity (part A), G1/G3-G5/G11 baseline quality (part C) and H1-H5 (hardware). `[hw]` items stay in their tasks' pending lists: the T-237 stealth-firewall cable run, the real 34" in T-123, and real drag and latency.

## Observations (not failures)

1. **Harness false negatives, fixed in this round's copy.** The first full run (`all.sh`) reused round 3's scripts and gave 11 FAIL/ERROR rows that are not app failures. Each was re-run after fixing the script:
   - Stale labels: "Remove lock" -> "Remove the lock", a leftover Spanish regex -> "Cursor locked here", "Update only" -> "Update automatically", a half-translated label -> "Measure bandwidth", "Crossed OK" -> "Crossed correctly" / "Crossed to ... through the right edge", "remote compute is missing" -> "remote compute does not respond".
   - `findings.mjs` checked a crossing without moving the mouse.
   - The Close-button hit test read `className` of an SVG.
   - `f15.mjs` expected an empty file where the agent writes `peer = []`.
   - `f16.mjs` polled too slowly for a fast copy (`f16b.mjs` uses a DOM observer).
   - `demo-sweep`, `misc` and `g2` ran without the demo bridge and pairs they need.

   The "step 5 shows the resolution" check was dropped: neither `F-Setup5` nor the app draws sizes there.
2. **`entered()` false positive.** When an earlier script leaves the cursor on the other machine, motion on the first machine is only forwarded there, and the harness's `entered()` counts that as a crossing. The crossing tests then wait for a crossing that never happens. `wiz5.mjs` and `f2cross.mjs` now bring the cursor home first, and the UI then reports the crossing every time. Engine and UI behave correctly. Only the harness was wrong.
3. **"Data cap" label on loopback.** On the one-way pair the dialling side logs `data cap link=Some(Wifi)` for the USB4 candidate on 127.0.0.1. This is the classification of a loopback address, the same artifact as round 3's observation 2. The T-237 pending list already asks for this check on the real cable.
4. **The `wifi` peer is the one dropped (T-223).** With `wifi` declared first and `ethernet` second on the same interface, the kept candidate is the `ethernet` one. That matches the T-223 notes ("best declared kind first").

## Fix tasks

| Task | Failure | Flow |
|---|---|---|
| T-250 | "Save and apply" lands on Monitors: Changes in progress leaves before the transaction's first notice arrives | F3, P9 |
| T-251 | "Update all": the command reply overwrites the step notices; the pane never shows the step and the button stays enabled | F13, P11 |

## Reproduce

1. Copy `acc_host.rs` to `crates/tandemd/examples/`, `acc_bridge.rs` to `apps/tandem-ui/src-tauri/examples/` and `acc_motion.rs` to `crates/tandem-hw/examples/`.
2. Build:
   - `cargo build -p tandemd --features sim-backends --bin tandemd --example acc_host`
   - `cargo build -p tandem-ui --example acc_bridge`
   - `cargo build -p tandem-hw --example acc_motion`
   - `cargo build -p tandem-cli`
3. Copy the five binaries to the directory that `bin()` in `harness/lib.mjs` reads. Run `pnpm -C apps/tandem-ui build` and `build:demo`.
4. Run the flows: `bash all.sh` (official run on tag `fin`), then `demo-up.sh` and `setup2.sh <tag> '<pair options>'` for the other pairs. Pairs used: `r2` (macOS+Windows), `win2`/`mac2` with `{"a":["windows",...],"b":["windows",...]}`, `ow` with `{"host":true,"env2":{"ACC_NO_DIAL":"1"}}`, and `bh` (paired, then `sink.mjs` as A's peer).
5. Run the round 4 scripts against those pairs: `t237.mjs`, `t240.mjs`, `t223.mjs`, `admin.mjs`, `f3prog.mjs`, `upd3.mjs`, `wiz5.mjs`, `f2cross.mjs`, `t166.mjs`, `t166b.mjs`, `g2.mjs`, `tray3.mjs`, `es.mjs`.

`WT` and `ACC_RUN` come from the environment. `ACC_RUN` must be a short path, because the agent socket path is limited to 108 bytes.
