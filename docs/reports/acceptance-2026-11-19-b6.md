# Tandem v0.3 acceptance - part B, round 6 (end-to-end flows)

Date: 2026-11-19. Base: `origin/develop` at `9fe53be` (T-291) for the T-280/T-291 checks and the first pass of every flow. While this round ran, `develop` gained `041e90f`/`80366c3` (part C report, T-320, T-321: docs and the root `package.json`), `dd9a3e4`/`2f31d4f` (part A report, T-300: mockup only) and `fe92474` (T-322: the start-up desk migration in `crates/tandemd/src/agent.rs`). The agent, bridge and UI were rebuilt on **`fe92474`**, and the official run plus samples of T-280 and T-291 were repeated on it (section "Official run on `fe92474`"). Protocol v41. Agent: acceptance (part B). Out of scope: pixel parity (part A) and baseline CI quality (part C).

## Verdict

**FAIL 1.** T-280 and T-291 are fixed. T-270 holds. Every flow passes on macOS+Windows, Windows+Windows and macOS+macOS except one timing requirement:

- **T-310**: cutting the primary path sometimes takes more than 1 s to move input to the next path (8 of 43 timed cuts, worst 1271 ms, also on `fe92474`). Network itself reports "switched to Ethernet in 1.2 s" or "in 1.3 s" after every cut. The health ping design (1 s interval + 250 ms timeout) allows up to 1.25 s, while DESIGN §6 and F6 ask for under 1 s. Flow F6.

## How it was tested

Round 5's harness, adapted (`docs/reports/acceptance/2026-11-19-b6/harness/`). Two real `tandemd` run over loopback. The host (`acc_host.rs`) runs doubles for the input engine, DDC and clipboard. One HTTP bridge per machine (`acc_bridge.rs`, which already carries the T-251 updater pieces) runs the app core's real code. The production UI runs in headless Chromium with Playwright. Demo mode runs on its own bridge. UDP proxies cut paths, and an MCP client runs against `tandem mcp`.

Harness changes:

- **`px.mjs` and `netcfg.mjs` give each path its own loopback address** (USB4 `127.0.0.2`, Ethernet `127.0.0.3`, Wi-Fi `127.0.0.4`), the way each real interface has its own IP. With every path on `127.0.0.1`, the agent cannot tell which path a connection opened by the other agent came over (T-237 adopts connections by IP), so it labelled the USB4 path with the other side's "Wi-Fi" class and refused "Measure bandwidth". That made F6 fail on 3 of 6 pairs (`fin`, `nw4`, `nw5`). With one address per path it passed on all 9 pairs that followed (`nw6`-`nw11`, `ww`, `mm`, `final`, `ww6`, `mm6`). Real interfaces never share an IP across paths, so this is a harness artifact (observation 1).
- `allp.sh <tag> [pair options]`: `all.sh` for any pair, so the whole suite runs on Windows+Windows and macOS+macOS too. Those pairs use the names "Mac" and "PC" with both systems the same, because many scripts select by name. G2 runs on separate pairs with neutral names (`win2`: Tower-1/Tower-2, `mac2`: Book-1/Book-2).
- `t250.mjs` takes `WIN=nonowner` to drive the window of the machine that does not own Std-34 (T-280), and its pass threshold follows N.
- `t280r.mjs` and `t280loop.sh`: the T-280 check on fresh pairs. `t291.mjs` and `t291loop.sh`: the T-291 check. `f6repro.sh` and `f6fail.mjs`: the F6 checks behind T-310.
- `f1c.mjs`/`f1d.mjs` open setup from Settings when it does not open by itself (a Windows `m1`). `wiz5.mjs`/`f2cross.mjs` move the cursor home away from the other machine whichever side `m1` is, and accept a crossing through the left edge.

Environment limits (unchanged): no WebKitGTK, no system tray or global shortcuts, the real updater replaced by a double, and everything `[hw]` left for the T-123 suite or each task's pending hardware list. Raw results are in `results.jsonl` (the whole session, including the harness false negatives listed under observations). Screenshots are in `docs/reports/acceptance/2026-11-19-b6/<flow>/`; each flow folder holds the last pair that ran it. The F5 Tunnels screenshots were removed, as in earlier rounds, because they show the test project's path.

## Round 5 failure and fixes since round 5

| Task | What was checked | Result |
|---|---|---|
| T-280 | 15 fresh macOS+Windows pairs (`r6-1`…`r6-15`; 12 on `9fe53be`, 3 on `fe92474`), each paired through the UI with `setup.sh` and the shared monitors plugged in right away (`t3.mjs`). `t280r.mjs` right after setup and again 30 s later: both agents hold the owner's report of Std-34 and Zfail-27, with the same owner, on **30 of 30** checks (round 5: missing on side B in 3 of 7 pairs). Then `WIN=nonowner t250.mjs` from the non-owner's window: 10 "Save and apply" + 10 card "Apply" on 5 pairs, 2 + 2 on 7 pairs and 4 + 4 on 3 pairs, **152 of 152** runs show Changes in progress and switch the owner's DDC bus. The non-owner was side B on `m2` in some pairs and on `m1` in others. (The 7 pairs with N = 2 logged "FALLA (2/2)" because the old script asked for at least 10 runs; every run passed, and the threshold now follows N.) (`T-280-scenes-from-non-owner/`) | PASS |
| T-291 | Pairing through the UI, then both agents sampled every second for 70 s (past `PAIRING_SETTLE`, 60 s), and both agent logs searched for "no longer trusts this one". **21 of 21 pairs** stay paired with a link in all 140 samples, and nothing is forgotten: 5 normal, 4 with `m1` unable to dial, 3 with `m2` unable to dial, 5 with every core busy during pairing, 3 busy with `m2` unable to dial, and 1 more (`ow`) after the full T-237 check from the side that cannot dial. Also every T-280 pair (15) and every flow pair stayed paired until "Leave desk". None of these runs logged "still saving the pairing", so the early-rejection path itself was not hit with real agents; it is covered by `a_rejection_before_the_other_saves_the_pairing_does_not_unpair` in CI. | PASS |
| T-270 | `t270.mjs` on `win2` and `mac2`: no "Allow remote execution" row on either machine page; the only switches are "Keyboard and mouse" and "Clipboard", and each one really turns the other machine's module off and on (`active -> off -> active`). No console errors. `t244.mjs` (the round 5 check of the old locked row) times out looking for that row, which is expected now that it is gone (`T-244-T-270-machine-switches/`). | PASS |
| T-237 with T-291 | Pair `ow` (B cannot dial): all 11 `t237.mjs` checks pass from B: paired with a link, no flapping, UI with no trouble state, ping, config changes both ways, crossing both ways, MCP tunnels both ways (`T-237-one-way-nodial-m2/`). | PASS |

## New failure

| Task | What was checked | Result |
|---|---|---|
| T-310 | `f6b.mjs` (official F6) on 18 fresh pairs: 15 under 1 s (309-955 ms), 3 over (1207 ms on `nw3`, 1241 ms on `nw10`, and 1271 ms on `mm6` on `fe92474`). `f6fail.mjs nw12 25`: 25 cuts at random phase, **5 of 25 at 1 s or more** (1001-1270 ms). Network shows "switched to Ethernet in 1.2 s" / "in 1.3 s" after every cut of the round: that is `took_ms`, measured by the agent from the dropped path's last answer. Cause: `HealthConfig` pings every 1 s with a 250 ms timeout, and its own test allows 1.5 s; DESIGN §6 and F6 require under 1 s. | **FAIL** |

## Official run on `fe92474`

`allp.sh final` (macOS+Windows), `allp.sh ww6` (Windows+Windows) and `allp.sh mm6` (macOS+macOS), each on a fresh pair: `final` 132 PASS / 2 FAIL, `ww6` 132 / 2, `mm6` 131 / 3. Two per pair are the same harness artifacts as rounds 4 and 5 (observation 2): P10 `f16.mjs` polls too slowly for a fast copy (`f16b.mjs` passes), and G6 lists radios that are already selected or disabled. The third on `mm6` is T-310: the primary cut took 1271 ms. The first pass on `9fe53be` (`fin`, `ww`, `mm`) gave the same result plus the F6 "Measure bandwidth" artifact on `fin` (observation 1).

## Verdict per item of `docs/ACCEPTANCE.md`

| Item | Verdict | Tasks | Evidence |
|---|---|---|---|
| F1 First use | PASS | | Setup opens by itself on macOS and from Settings on Windows. The same 6-digit code shows on both UIs, a wrong code is rejected, and the pair ends with different sides, also when one side cannot dial (`ow`, T-291 pairs). Step 4 detects the hub and marks activity on the other machine in both windows (`f1d.mjs` on `final`, `ww2`, `mm2`; `t166.mjs` on `win2`, `mac2`). Step 5: a real engine crossing enables Continue and the summary says "crossing tested" (`wiz5.mjs` on `win2`, `mac2`, with `m1` on side B). The MCP command is copied. Permissions are doubles (`[hw]` real) |
| F2 Arrangement | PASS | | 4 sides x 3 alignments reach both machines and `tandem.toml`. The engine crosses through the new edge and not through the old one (all three pairs). The Arrangement crossing test gives "Crossed correctly" with a real crossing and "Did not cross in 20 s" at 20 s (`f2cross.mjs` on `win2`, `mac2`) |
| F3 Scenes | PASS | | Create, edit, apply, injected failure with visible rollback and the global shortcut, on all three pairs. From the non-owner: 152 of 152 (T-280) |
| F4 Monitors | PASS | | Factory monitor "Unsupported" with zero DDC writes; input change with a verdict; owner re-election when the owner powers off. Both sides hold the owner's report on every fresh pair (T-280). `[hw]` 34" stays in T-123 |
| F5 Tunnels through MCP | PASS | | All checks on all three pairs: tools, git workspace, credentials filtered, secret env refused, 3 processes at once, output, files in and out, port forward, cut and recover without losing output, `bring_back`, a freshly opened UI shows the tunnel. Tunnels both ways from a side that cannot dial (`ow`) |
| F6 Network | **FAIL** | T-310 | Failover over 1 s on 8 of 43 timed cuts (`mm6` on `fe92474` included), and Network reports 1.2-1.3 s every time. The rest passes: the event is visible, the primary takes input back, "Measure bandwidth" gives a value on the carrying path (9 of 9 pairs with one address per path) |
| F7 Keyboard and mouse without an owner | PASS | | The machine in use is in charge; no stuck keys on an owner change or a link drop (all three pairs) |
| F8 Clipboard and files | PASS | | Text both ways and an image; the file copy notice shows progress and "Copied" (`f16b.mjs` on `win2`, `mac2`, `fx`). Drag is covered by the CI doubles; real drag is `[hw]` |
| F9 Shared configuration | PASS | | Rename, color, lock shortcut, Cmd<->Ctrl and the clipboard switch reach the other machine; "Changed from ... Undo" restores; a simultaneous rename converges (all three pairs, also on `fe92474` after T-322) |
| F10 Power | PASS | | Power off and on (simulated Wake-on-LAN) with steps on both agents; a lost machine shows its trouble state |
| F11 Modules | PASS | | The other machine's Clipboard and Keyboard-and-mouse switches work from its page; "unavailable" reasons correct (all three pairs, `win2`, `mac2`) |
| F12 Settings and palette | PASS | | Every palette command, "Lock here"/"Remove the lock", the window shortcut, "Measure the link", and "Leave desk" unpairs both |
| F13 Updates | PASS | | Version lookup against a test `latest.json`; "Update all" asks the other machine first, shows its step and keeps the button disabled; "did not come back" is reported; a 404 gives "could not check"; trouble states with their actions |
| F14 Tray | PASS | | State, a scene applied from the tray, "Open" and "Quit" (`final`, `ww6`, `mm6`). `tray3.mjs` on `win2`/`mac2`: "1 tunnel" per machine and nothing names the other system |
| G2 Nothing fixed to Mac/PC | PASS | | `g2.mjs` on `win2` and `mac2`: no screen or palette command names the other system or uses its shortcuts. Demo `win-win`/`mac-mac` in the demo sweep |
| G6 Every element does something real | PASS | | Button sweep on every live screen of all three pairs. The only "no effect" results are radios already selected (`aria-checked=true`: the input a monitor is on, the machine's own color, the selected screen with `aria-pressed=true`) or disabled (the color the other machine uses), checked one by one on `win2` |
| G7 / G8 Animations and reduced motion | PASS | | Demo: animated elements on Desk; nothing animated with `prefers-reduced-motion: reduce` |
| G9 Language and console | PASS | | `es.mjs` on `win2`, `mac2`, `fx` (every live screen, Settings tab and wizard step) and the demo sweep: no Spanish, no `{{`, `undefined` or `NaN`, no console errors |
| G10 Everything editable is edited from the UI | PASS | | The Wake-on-LAN MAC and the desk name reach both `tandem.toml` files |

Not evaluated here: P1-P15 pixel similarity (part A), G1/G3-G5/G11 baseline quality (part C) and H1-H5 (hardware). `[hw]` items stay in their tasks' pending lists.

## Observations (not failures)

1. **"Measure bandwidth" refused on a loopback harness.** On `fin`, `nw4` and `nw5` the click got "Bandwidth is not measured over Wi-Fi". With every proxy on `127.0.0.1`, `Conns::inbound` (T-237) matches the other agent's connections by IP only, so it can adopt the Wi-Fi connection as the USB4 path, and the other side's `LinkClass` makes the path's label Wi-Fi. Real interfaces have different IPs, so the harness now gives each path its own loopback address, and the check passed on every pair after that.
2. **Known harness artifacts in the official runs**, the same as rounds 4 and 5: P10 `f16.mjs` polls too slowly (`f16b.mjs` passes), and the G6 sweep lists radios that are already selected or disabled.
3. **Harness false negatives during the round**, each fixed and re-run. They stay in `results.jsonl`:
   - A first Windows+Windows run with neutral names (`win2`) stopped in F3 because the scripts pick radios by the names "Mac" and "PC". The suite then ran with those names on both systems (`ww`, `ww6`, `mm`, `mm6`).
   - `f1c.mjs`/`f1d.mjs` waited for setup to open by itself on a Windows `m1`.
   - `wiz5.mjs`/`f2cross.mjs` moved the cursor "home" to the left, which crosses to the other machine when `m1` is side B, and expected "through the right edge".
   - `t250.mjs` with N = 2 required at least 10 runs.
4. **The step-5 resolution check is still dropped**, as in rounds 4 and 5. Neither `F-Setup5` nor the app draws sizes there.
5. The follow-up scripts on `win2`/`mac2` (`g2`, `tray3`, `t166`, `wiz5`, `f2cross`, `t270`, `es`, `f16b`) and most T-310 measurements ran on `9fe53be`. The only agent change in `fe92474` is T-322 (start-up desk migration), and the full suite on `fe92474` passes on all three pairs.

## Fix tasks

| Task | Failure | Flow |
|---|---|---|
| T-310 | Path failover can take more than 1 s (1 s health ping + 250 ms timeout) | F6, P7 |

## Reproduce

1. Copy `acc_host.rs` to `crates/tandemd/examples/`, `acc_bridge.rs` to `apps/tandem-ui/src-tauri/examples/` and `acc_motion.rs` to `crates/tandem-hw/examples/`.
2. Build:
   - `cargo build -p tandemd --features sim-backends --bin tandemd --example acc_host`
   - `cargo build -p tandem-ui --example acc_bridge`
   - `cargo build -p tandem-hw --example acc_motion`
   - `cargo build -p tandem-cli`
3. Copy the binaries to the directory that `bin()` in `harness/lib.mjs` reads. Run `pnpm -C apps/tandem-ui build` and `build:demo`, then `demo-up.sh`.
4. Official run: `allp.sh final`, `allp.sh ww6 '{"host":true,"a":["windows","Mac","#7aa7e6"],"b":["windows","PC","#a99be8"]}'` and the same with `macos` for `mm6`.
5. T-280: `t280loop.sh <first> <last> <N>`. T-291: `t291loop.sh <first> <last> <normal|nodial1|nodial2|load|loadnodial>`. T-310: `f6repro.sh <tag>`, or `setup.sh <tag>`, `px.mjs`, `netcfg.mjs`, `t3.mjs` and then `f6fail.mjs <tag> 25`.
6. G2 and the per-pair scripts: `setup2.sh win2 '{"host":true,"a":["windows","Tower-1","#7aa7e6"],"b":["windows","Tower-2","#a99be8"]}'` (and `mac2` with `macos`), `t3.mjs`, then `followup.sh`.

`WT` and `ACC_RUN` come from the environment. `ACC_RUN` must be a short path, because the agent socket path is limited to 108 bytes.
