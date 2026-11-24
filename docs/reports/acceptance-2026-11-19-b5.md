# Tandem v0.3 acceptance - part B, round 5 (end-to-end flows)

Date: 2026-11-19. Base: `origin/develop` at `95af721` (T-244), which includes T-250/T-251 (`cd83010`) and T-243/T-245 (`442614e`). While this round ran, `develop` gained `985403a` and `b9b21e4` (docs only) and `8cda900` (T-270, UI only: it removes the "Allow remote execution" row). The UI was rebuilt on `8cda900` and the machine pages were checked again on it (section "T-244 and T-270"). Agent and app-core code are the same in both. Protocol v41. Agent: acceptance (part B). Out of scope: pixel parity (part A) and baseline CI quality (part C).

## Verdict

**FAIL 1.** Every round 4 failure is fixed: T-250 passed 64 of 64 runs, and T-251 passed 3 of 3 runs with the pane sampled every 100 ms. T-243, T-244/T-270, T-237 and T-240 all pass. One new failure:

- **T-280**: right after pairing, the machine that does not own a monitor can lose the owner's report. Then every scene applied from that machine fails with "it is not known which machine is shown" (3 of 7 fresh pairs). Flows F3 and F4.

## How it was tested

Round 4's harness, adapted (`docs/reports/acceptance/2026-11-19-b5/harness/`). Two real `tandemd` run over loopback. The host (`acc_host.rs`) runs doubles for the input engine, DDC and clipboard. One HTTP bridge per machine (`acc_bridge.rs`) runs the app core's real code. The production UI runs in headless Chromium with Playwright. Demo mode runs on its own bridge. UDP proxies cut paths, and an MCP client runs against `tandem mcp`.

Harness changes:

- **`acc_bridge.rs` follows T-251.** Its copy of the updater now numbers every change of the update view (`seq`, with `update::renumber` in its `mutate`). "Update all" takes the install with `update::take_install`, so the reply already carries the first step. It also emits that view as an event, the same as `updater.rs`.
- New scripts:
  - `t250.mjs`: N "Save and apply" runs and N card "Apply" runs, each in a fresh window. It checks the progress heading, the owner's DDC bus and the console.
  - `upd5.mjs`: "Update all" with the pane sampled every 100 ms.
  - `t244.mjs` and `t270.mjs`: machine-page switches, live and demo.
  - `t280.mjs` and `t280b.mjs`: the T-280 repro and capture.
  - `mon.mjs`: the monitor owner and report on both agents.
- `t223.mjs` checks the T-243 wording, icon and tokens.

Pairs: macOS+Windows (`fin` for the official run, then `r2`, `r3`, `r4`, `rp1`, `rp2`, `q1`, `q2`), Windows+Windows (`win2`), macOS+macOS (`mac2`). Two pairs check T-237: `ow` (B cannot dial) and `bh` (A's peer address is a UDP sink). Raw results are in `results.jsonl`. It holds the whole session, including the harness false negatives listed under observations. Screenshots are in `docs/reports/acceptance/2026-11-19-b5/<flow>/`. The F5 Tunnels screenshots were removed, as in rounds 3 and 4, because they show the test project's path.

Environment limits (unchanged): no WebKitGTK, no system tray or global shortcuts, the real updater replaced by a double, and everything `[hw]` left for the T-123 suite or each task's pending hardware list.

## Round 4 failures, checked again

| Task | What was checked | Result |
|---|---|---|
| T-250 | `t250.mjs`: "Save and apply" from the scene editor in a fresh window, alternating the machine Std-34 shows, so each run is a real transaction. Then "Apply" on a scene card, also in a fresh window. Each run checks that "Changes in progress" is rendered (MutationObserver on the heading), that the owner's DDC double switched input, and that the console has no errors. Results: `r3` (macOS+Windows, from side B) 12 + 12, `win2` 10 + 10, `mac2` 10 + 10. **64 of 64** show the progress and switch the monitor. The official `f3.mjs` (create, edit, apply, injected failure with rollback, global shortcut) passes too (`T-250-save-and-apply/01-save-and-apply-progress.png`). | PASS |
| T-251 | `upd5.mjs`, 3 runs on `r3` against a test `latest.json` (9.9.9). The other machine never comes back in this double. "The other machine is updating; this one keeps the keyboard and mouse…" shows 121-142 ms after the click. It stays in each of the ~190 samples for the 20 s the run lasts. "Update all" is disabled in every sample. A forced second click changes nothing, with no "already in progress" error. At the end "the other machine did not come back" shows and the button is enabled again. The other agent logs `the other machine asks to install a new version version=9.9.9`. No console errors (`T-251-update-all/01-update-all-running.png`, `02-update-all-ended.png`). | PASS |

## What changed since round 4

| Task | What was checked | Result |
|---|---|---|
| T-243 | `t223.mjs` on `r3`. Both agents restart with a Wi-Fi and an Ethernet peer on the box's real `eth2`. Network shows "Ethernet path · Two addresses reach the other machine over the Ethernet path. Tandem uses [fe80::…%5]:<port> and ignores 192.168.1.172:<port>." The notice has the warn icon, background `#1e1a12`, border `#5a4a2c` and text `#e6c48f` (the tray's warning tokens, `Tray.svelte`). It does not mention `tandem.toml`, `[network]` or removing a peer, and it starts with a capital. The link stays up (ping over Ethernet, 1 ms) and there are no console errors (`T-223-two-peers-one-interface/01-network.png`). The "Link manager" notice cannot be reached from the UI or the config any more (duplicate peers are merged by T-223), so its wording is covered by the unit tests in `notices.rs` only. | PASS |
| T-244 (on `95af721`) | `t244.mjs` on `r3` and `win2`. "Allow remote execution" on the other machine's page is `aria-readonly`, not natively disabled, opacity 1, focusable. Its tooltip and accessible description say "Always on: remote execution can't be turned off from here.". A click, Space and Enter send no command, and `aria-checked` and the tunnels module stay the same on both agents. Only "Keyboard and mouse" and "Clipboard" are real switches. Demo: `win-win` and `mac-mac` with notices name only that pair's machines. No Permissions card with "Open System Settings" on a Windows machine. The admin note with `admin=off` is the macOS one for a macOS machine (`?demo`, `?demo=mac-mac`) and the Windows one for a Windows machine (`?demo&here=b`, `?demo=win-win`) (`T-244-locked-switch/`). | PASS |
| T-244 and T-270 (on `8cda900`) | `t270.mjs` on `q2` with the UI rebuilt on `8cda900`. There is no "Allow remote execution" row on either machine page. The only switches are "Keyboard and mouse" and "Clipboard", and each one really turns the other machine's module off and on (`active -> off -> active`). The same holds in demo mode for `mac-win`, `win-win` and `mac-mac`, machines `a` and `b`. No console errors (`T-244-T-270-machine-switches/01-other-machine-page.png`). | PASS |
| T-237 (no dial) | Pair `ow`: B starts with `ACC_NO_DIAL=1`. Pairing through the UI works. Checked from B (`t237.mjs ow m2`): both agents are paired over USB4, with no drop in 12 s of samples. B's Desk and Network show the link up with no trouble state and no console errors. Ping answers in 1 ms. A rename reaches the other machine in each direction. Keyboard and mouse cross both ways. An MCP tunnel from B runs on A, and one from A runs on B (`T-237-one-way-nodial-m2/`). | PASS |
| T-237 (stealth firewall) | Pair `bh`: after pairing, A's peer address points to `sink.mjs` and A restarts. The sink saw only 4 packets: A adopted B's connection. Every check above passes from A (`t237.mjs bh m1 blackhole`, `T-237-one-way-blackhole-m1/`). | PASS |
| T-240 | `t240.mjs ow m2`: both windows on step 4, then B restarts. The key is heard on B while B's own link is not installed yet. A's window marks B's row "Activity detected" once the link appears. The baseline shows nothing before the key (`T-240-key-probe-replay/01-other-window.png`). | PASS |

## New failure

| Task | What was checked | Result |
|---|---|---|
| T-280 | On 7 fresh macOS+Windows pairs, `mon.mjs` was run right after setup. Side B (the non-owner) had `report: null` for every shared monitor on 3 of them (`r2`, `rp1`, `q2`), while the owner had a report. On `r2`, 24 of 24 scene applies from B's window failed with "it is not known which machine is shown…", and the monitor did not move. The same scene from A worked, and after that B worked again. On `q2`, Changes in progress shows the failure (`T-280-report-lost-on-owner-change/02-apply-from-non-owner.png`). Likely cause: a race at monitor discovery overwrites the owner's report with B's own read, and the owner never resends it (details in the task). | **FAIL** |

## Verdict per item of `docs/ACCEPTANCE.md`

Official run: `all.sh` on the fresh pair `fin`, 134 checks, 132 PASS. The other 2 are harness artifacts, re-run below (observation 1).

| Item | Verdict | Tasks | Evidence |
|---|---|---|---|
| F1 First use | PASS | | Setup opens by itself. The same 6-digit code shows on both UIs, and a wrong code is rejected. The pair ends with different sides, also when one side cannot dial. Step 4 marks the other machine in both windows (`t166.mjs`, `t166b.mjs` on `mac2`, T-240). Step 5: a real engine crossing enables Continue (`wiz5.mjs` on `win2`). The summary says "crossing tested", and the MCP command is copied. Permissions are doubles (`[hw]` real) |
| F2 Arrangement | PASS | | 4 sides x 3 alignments reach both machines and `tandem.toml`. The engine crosses through the new edge and not through the old one. The Arrangement crossing test gives "Crossed correctly" with a real crossing and "Did not cross in 20 s" at 20 s (`fin`, `win2`) |
| F3 Scenes | **FAIL** | T-280 | Create, edit, apply, injected failure with visible rollback, and the global shortcut all pass. The T-250 progress fix holds (64 of 64). But on 3 of 7 fresh pairs, applying from the machine that does not own the monitor fails until the owner's input changes |
| F4 Monitors | **FAIL** | T-280 | Factory monitor: "Unsupported" with zero DDC writes. The input change shows a verdict. Owner re-election works when the owner powers off. The non-owner's missing report (T-280) leaves it unable to change a monitor. `[hw]` 34" stays in T-123 |
| F5 Tunnels through MCP | PASS | | All 19 checks: tools, git workspace, credentials filtered, secret env refused, 3 processes at once, output, files in and out, port forward, cut and recover without losing output, `bring_back`, a freshly opened UI shows the tunnel. Tunnels also work from the side that cannot dial (T-237) |
| F6 Network | PASS | | A primary cut moves input to the next path in under 1 s. The event is visible, the primary takes input back, and "Measure bandwidth" gives a value. T-243 notice as above |
| F7 Keyboard and mouse without an owner | PASS | | The machine in use is in charge. No stuck keys on an owner change or a link drop |
| F8 Clipboard and files | PASS | | Text A->B and B->A, and an image A->B. The file copy notice shows progress and "Copied" (`f16b.mjs`). Drag is covered by the CI doubles; real drag is `[hw]` |
| F9 Shared configuration | PASS | | Rename, color, lock shortcut, Cmd<->Ctrl and the clipboard switch reach the other machine. "Changed from ... Undo" restores. A simultaneous rename converges (`fin`, `win2`, `mac2`) |
| F10 Power | PASS | | Power off and on (simulated Wake-on-LAN) with steps on both agents. A lost machine shows "PC is off · Power on PC" |
| F11 Modules | PASS | | The other machine's Clipboard and Keyboard-and-mouse switches work from its page. The "unavailable" reasons are correct (`fin`, `win2`, `mac2`, `q2` on `8cda900`) |
| F12 Settings and palette | PASS | | Every palette command and its navigation. "Lock here" and "Remove the lock". The window shortcut is saved. "Leave desk" unpairs both (`peer = []`) |
| F13 Updates | PASS | | Version lookup against a test `latest.json`. "Update all" asks the other machine first and shows its step, with the button disabled (T-251). When the other machine does not come back, the UI says so. A 404 gives "could not check". "Update automatically" is saved in `update.toml`. Trouble states work with their actions |
| F14 Tray | PASS | | State, a scene applied from the tray, "Open" and "Quit". Windows+Windows and macOS+macOS: "1 tunnel" per machine, and nothing names the other system |
| G2 Nothing fixed to Mac/PC | PASS | | `g2.mjs` on `win2` and `mac2`: no screen names the other system or uses its shortcuts. Demo `win-win`/`mac-mac` (T-244) |
| G6 Every element does something real | PASS | | Button sweep on every live screen. The only "no effect" result is the input radio a monitor is already on ("PC DP", `aria-checked=true`), as in round 4. The locked switch (T-244) is gone with T-270 |
| G7 / G8 Animations and reduced motion | PASS | | Demo: animated elements on Desk; nothing animated with `prefers-reduced-motion: reduce` |
| G9 Language and console | PASS | | Every live screen, Settings tab and wizard step (`es.mjs`), and the demo sweep for all pairs and states: no Spanish, no `{{`, `undefined` or `NaN`, and no console errors. Agent logs are in English |
| G10 Everything editable is edited from the UI | PASS | | The Wake-on-LAN MAC and the desk name reach both `tandem.toml` files |

Not evaluated here: P1-P15 pixel similarity (part A), G1/G3-G5/G11 baseline quality (part C) and H1-H5 (hardware). `[hw]` items stay in their tasks' pending lists.

## Observations (not failures)

1. **Two official-run rows re-run.**
   - P10: `f16.mjs` polls too slowly for a fast copy. `f16b.mjs` (DOM observer) passes on `r2`, `r3` and `r4`.
   - G6 "PC DP": that radio is the input the monitor is already on.

   Both are the same as round 4.
2. **Harness false negatives during the round**, each fixed and re-run. They stay in `results.jsonl`:
   - `t250.mjs` read the DDC double of a fixed host. Each host has its own double, and only the owner's moves. It also reused scene names, and it used machine names that F9 had changed.
   - `upd5.mjs` counted samples taken in the first ~20 ms after the click, before the reply reached the window.
   - `t244.mjs` flagged "Ctrl+K" in `mac-mac`. That label comes from the browser the test runs in (`isMacPlatform`), not from the demo pair.
   - The T-280 rows on `r2` were first logged as T-250 failures, because the monitor never moved. The agent log shows the transaction failing on the missing report, while the progress screen was shown 24 of 24 times.
3. **One pairing lost to a port clash.** On `rp3` one agent exited at startup with `Address already in use`, because a port the harness picked was taken by one of the several pairs alive at once, so pairing timed out. This is a harness artifact.
4. **The step-5 resolution check is still dropped**, as in round 4. Neither `F-Setup5` nor the app draws sizes there.

## Fix tasks

| Task | Failure | Flow |
|---|---|---|
| T-280 | The non-owner can lose the owner's monitor report at pairing; scenes applied from it then fail | F3, F4, P9 |

## Reproduce

1. Copy `acc_host.rs` to `crates/tandemd/examples/`, `acc_bridge.rs` to `apps/tandem-ui/src-tauri/examples/` and `acc_motion.rs` to `crates/tandem-hw/examples/`.
2. Build:
   - `cargo build -p tandemd --features sim-backends --bin tandemd --example acc_host`
   - `cargo build -p tandem-ui --example acc_bridge`
   - `cargo build -p tandem-hw --example acc_motion`
   - `cargo build -p tandem-cli`
3. Copy the five binaries to the directory that `bin()` in `harness/lib.mjs` reads. Run `pnpm -C apps/tandem-ui build` and `build:demo`.
4. Run the flows. `bash all.sh` is the official run on tag `fin`. Then run `demo-up.sh`, and `setup.sh <tag>` or `setup2.sh <tag> '<pair options>'` for the other pairs:
   - `win2`: `{"host":true,"a":["windows","Tower-1","#7aa7e6"],"b":["windows","Tower-2","#a99be8"]}`
   - `mac2`: the same with `macos`.
   - `ow`: `{"host":true,"env2":{"ACC_NO_DIAL":"1"}}`
   - `bh`: a normal pair, then `sink.mjs` as A's peer and a restart of A.
5. Run this round's scripts against those pairs: `t250.mjs <tag> 10`, `upd5.mjs <tag> 3`, `t244.mjs`, `t270.mjs`, `t223.mjs`, `t237.mjs`, `t240.mjs`, `g2.mjs`, `t166.mjs`, `t166b.mjs`, `es.mjs`, `tray3.mjs`, `wiz5.mjs`, `f2cross.mjs`, `f9.mjs`, `f11.mjs` and `f16b.mjs`.
6. For T-280: run `setup.sh <tag>` and then `mon.mjs <tag>`. If side B shows `report: null`, run `t280b.mjs <tag>`. It reproduced on about 1 pair in 3.

`WT` and `ACC_RUN` come from the environment. `ACC_RUN` must be a short path, because the agent socket path is limited to 108 bytes.
