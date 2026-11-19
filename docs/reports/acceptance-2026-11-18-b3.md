# Tandem v0.3 acceptance - part B, round 3 (end-to-end flows)

Date: 2026-11-18 - Base: `origin/develop` at `bbdc245` (fast-forwarded to `54ce1df`, which only adds the part A and C reports, tasks and CI timeouts: no code change) - Agent: acceptance (part B). Out of scope: pixel parity (part A) and baseline CI quality (part C).

## Verdict

**FAIL.** T-166 and T-167 (the two round 2 failures) are fixed in the cases they described, and the new work (T-196 link classification, T-197 absolute data dir, T-209 persisted-id migration, the English UI) works end to end. Four new failures were found: **T-220** (second machine's activity in setup step 4 is not forwarded), **T-221** (mistranslated label "Update only"), **T-222** (Spanish tracing fields in the agent log) and **T-223** (relabelled links can collide and leave the agent without any link).

## How it was tested

Same harness as rounds 1 and 2 (`docs/reports/acceptance/2026-11-18-b/harness/`, `.../2026-11-18-b2/harness/`), adapted to the English UI and extended (`docs/reports/acceptance/2026-11-18-b3/harness/`): two real `tandemd` over loopback (the host with doubles for input engine, DDC and clipboard where the flow needs them), one HTTP bridge per machine running the app's real code, the production UI in Chromium with Playwright, UDP proxies to cut paths, an MCP client against `tandem mcp`. Pairs: macOS+Windows, Windows+Windows, macOS+macOS; demo mode with all routes and variants. Screenshots in `docs/reports/acceptance/2026-11-18-b3/<flow>/` (the Tunnels ones were removed: they show the test project's path). Raw results: `results.jsonl` (the whole session, including false negatives of the first, partly stale harness runs, see below).

Environment limits (unchanged): no WebKitGTK, no system tray or global shortcuts, real updater replaced by a double, everything `[hw]` left for the T-123 suite or each task's pending hardware list. `ACC_RUN` has to be a short path (the agent socket is limited to 108 bytes), so the run directory is outside the scratchpad.

## Round 2 failures, checked again

| Task | What was checked now | Result |
|---|---|---|
| T-166 | Two windows on step 4: activity (key + mouse move) on B marks B's row in A's window; A's own row stays unmarked in A's window; the baseline shows nothing before pressing; vice versa when A is first. | PASS for the first activity. **FAIL** for the second one: after A was heard, activity on B never reaches A's window (and the reverse), so the windows end up disagreeing (`t166-order-window-A.png`, `t166-order-window-B.png`) -> **T-220** |
| T-167 | Tray with one tunnel: "1 tunnel" (singular); each machine row shows its count; Windows+Windows and macOS+macOS pairs with one tunnel to each machine: no fixed Mac/PC text, "1 tunnel" on both rows, same on both windows (`F14-tray-T167/`). | PASS |

## What changed since round 2

| Task | What was checked | Result |
|---|---|---|
| T-196 | Real interfaces of the box (`tandemd links`: `eth1` has a link-local IPv4 and an `fe80::` address). A peer declared `wifi` at the IPv4 link-local address is reported as Ethernet (primary, 0.6 ms) and connects. A peer at the IPv6 link-local address with the interface's scope id connects (dual-stack socket `*:port` confirmed with `ss`), no `InvalidRemoteAddress`. `cargo test -p tandem-link`: 165 + 13 + other tests green (simulated interface tables for both platforms, scope id, QoS profile). Edge case found: two configured peers on the same interface -> `the link Ethernet is configured twice` and no link at all | PASS for the task's criteria; edge case -> **T-223** |
| T-197 | Both agents started with relative `--data-dir` and `--config` (cwd = machine root, no XDG variables): they come up paired, `tunnel_open` with a git workspace works, the project arrives under the data dir, a commit made in the tunnel comes back with `bring_back` (`tandem/rel` branch in the origin project). | PASS |
| T-209 | An old config with the three default scenes, the old icon, a user scene, old screen ids (also in `[[cables]]`) and an old stored state: the agent loads the English ids (`work`, `only-a`, `only-b`, `screen-1`), translates the default names, keeps the user scene's name, rewrites the file once, moves the stored scene id, a second load leaves the file byte-identical and the UI shows "Work / Only Mac / Only PC". The live flows (scenes, tray, power state) use the new ids. | PASS |
| English UI (T-201, T-202) | All live screens, every Settings tab and every wizard step on both machines (37 pages) plus the demo sweep (3 pair variants x 12 routes + 14 states): no Spanish, no `{{`, `undefined`, `NaN`, no console errors. Two mistranslated labels found (T-221). | PASS for language; **FAIL** for sense -> **T-221** |

## Verdict per item of `docs/ACCEPTANCE.md`

| Item | Verdict | Tasks | Evidence |
|---|---|---|---|
| F1 First use | **FAIL** | T-220 | Pass: setup opens by itself, same 6-digit code on both UIs, wrong code rejected, paired with different sides, keyboard detection on a machine with and without hub, step 5 with a real crossing ("Crossed to PC through the right edge, as the arrangement says", Continue enabled), summary "crossing tested" and the copyable MCP command. Fails: the second machine's activity in step 4 |
| F2 Arrangement | PASS | | 4 sides x 3 alignments reach both machines and `tandem.toml`; the engine crosses through the new edge and not through the old one |
| F3 Scenes | PASS | | create, edit, apply with progress, apply with an injected failure -> visible rollback and the desk back as it was; global scene shortcut |
| F4 Monitors | PASS | | factory monitor "Unsupported" with its reason and no DDC write; input change with the monitor's verdict; owner re-election when the owner is killed. `[hw]` 34" with automatic revert stays in T-123 |
| F5 Tunnels through MCP | PASS | | tools list, git workspace, credentials filtered, secret env vars refused, 3 processes at once, output without mixing, files in and out, port forward, cut and recover the link without losing output, `bring_back`; the freshly opened UIs show the tunnels |
| F6 Network | PASS | | primary cut -> next path in 788 ms, event visible, primary takes input back; "Measure bandwidth" gives a value over USB-C and Ethernet (see observation 2) |
| F7 Keyboard and mouse without an owner | PASS | | the one in use is in charge; no stuck keys on owner change or link drop |
| F8 Clipboard and files | PASS | | text A->B, B->A and image; file copy shows a notice with progress ("Sending ... 0 % ... 524 MB") |
| F9 Shared configuration | PASS | | rename, color, lock shortcut, preferences, module switch reach the other machine; "Changed from ... Undo" restores; a simultaneous rename converges |
| F10 Power | PASS | | power off and on (simulated Wake-on-LAN) through the palette, steps recorded by both agents |
| F11 Modules | PASS | | module switches of the other machine work from its page; "unavailable" reasons correct |
| F12 Settings and palette | PASS | | palette commands and navigation, Ctrl+, , "Open setup", "Change the other machine...", "Measure the link", "Lock here"/"Remove the lock", "Leave desk" with confirmation (both trust files end empty), window shortcut saved in `ui.toml` |
| F13 Updates | PASS | | version lookup against a test `latest.json`, "Update all" asks the other machine first and the UI says it is waiting, a 404 shows "could not check: ... 404"; trouble states (permissions, link down, other machine off, no agent) with their actions. The label of the automatic switch is wrong (T-221) |
| F14 Tray | PASS | | state, scene applied from the tray reaches the agent, "Open" and "Quit" reach the app core |
| G2 Nothing fixed to Mac/PC | PASS | | Windows+Windows and macOS+macOS pairs: no screen, palette or tray names the other system |
| G6 Every element does something real | PASS | | button sweep on every live screen: the only "no effect" results are the active tab, the disabled fixed shortcut, and two false negatives of the sweep (Measure bandwidth takes 3 s; Save in Arrangement shows "Arrangement saved"), both checked by hand |
| G7 / G8 Animations and reduced motion | PASS | | demo: 39 animated elements on Desk; no animation or transition with `prefers-reduced-motion: reduce` on 6 screens; part A covers parity |
| G9 Language and console | **FAIL** | T-221, T-222 | No Spanish text and no console errors on the screens; but "Update only" (and "Band") read wrongly and two Spanish field names are still written to the log |
| G10 Everything editable is edited from the UI | PASS | | Wake-on-LAN MAC and machine name saved in `tandem.toml` of both machines |

Not evaluated here: P1-P15 pixel similarity (part A), G1/G3-G5/G11 baseline quality (part C) and H1-H5 (hardware). `[hw]` pending items of T-196 and T-197 (real cable run, `GetAdaptersAddresses` description, IPv6 over the cable, `tunnel-workspace` in the next cable run) stay in their tasks.

## Observations (not failures)

1. **Harness false negatives.** The first runs of this session used a harness still carrying stale labels (Spanish words replaced one by one in round 2): 21 rows of `results.jsonl` say FAIL. Each was re-run or checked by hand after fixing the script: stale labels (Remove lock, Confirm, Updates tab, "Measure bandwidth", sidebar link), a scene shortcut already taken by a scene created in an earlier run, monitors not re-added after an agent restart, a notice poller that missed a fast transfer (re-done with a DOM observer), the wrong parameter name of `bring_back` in my own T-197 script. The only rows that stay as real findings are the T-166 "vice versa" one (T-220).
2. **Loopback proxies and the bandwidth test.** With three paths proxied through loopback on different ports, the agents sometimes label the USB-C path as Wi-Fi after a restart (the inbound side classifies by the source address, which is the same for the three proxies) and "Measure bandwidth" is then refused with "Bandwidth is not measured over Wi-Fi..." (a visible notice) while the card still says USB-C. The measurement works when the labels are right. It is an artifact of the test setup, but it shows that the card and the refusal can disagree; not a failure of any criterion.
3. **Step 4 semantics.** `Agent::hear` is deliberately "first key wins" (T-166 notes). T-220 asks for the per-machine version because the step lists both machines ("keyboard and mouse detected on each machine").


## Fix tasks

| Task | Failure | Flow |
|---|---|---|
| T-220 | Setup step 4: only the first machine heard is forwarded; the windows disagree afterwards | F1 |
| T-221 | "Update only" for the automatic-update switch ("Band" and "Main" too) | G9, P11 |
| T-222 | Two Spanish tracing field names in the agent log | G9 |
| T-223 | Relabelled link candidates can collide; the link manager refuses to start and the agent has no link | T-196 |

## Reproduce

Build `tandemd` (feature `sim-backends`), `tandem`, and the two examples `acc_host` and `acc_bridge` (sources in `docs/reports/acceptance/2026-11-18-b/harness/`, copied to `crates/tandemd/examples/` and `apps/tandem-ui/src-tauri/examples/`), copy the four binaries to the directory `bin()` of `harness/lib.mjs` reads, `pnpm -C apps/tandem-ui build` and `build:demo`, then `node up.mjs <tag> '{"host":true}'`, `node f1.mjs <tag>`, `node fin.mjs <tag>`, `node t3.mjs <tag>` and the flow scripts (`f2`, `f3`, `f4`, `f5`, `f6a`, `f6b`, `f9`...`f16b`, `t166.mjs`, `t166b.mjs`, `t196.mjs v4|v6`, `t197.mjs`, `mig.mjs`, `tray3.mjs`, `es.mjs`, `demo-sweep.mjs`). `WT` and `ACC_RUN` come from the environment (`ACC_RUN` short, for the socket path). Environment note: the lockfile install works here (`pnpm install --frozen-lockfile`).
