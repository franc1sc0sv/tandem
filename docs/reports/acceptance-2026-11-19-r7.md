# Tandem v0.3 acceptance, round 7 (parts A, B and C): final check after the round 6 fixes

Date: 2026-11-19. Base: `origin/develop` at `22674c3` (tag `v0.3.0`), which adds to round 6's bases T-300 (`2f31d4f`, mockup), T-322 (`fe92474`, agent), T-310 (`0a9a8a6`, link manager) and the version bump (`22674c3`). Branch `qa/acceptance-2026-11-19-r7`. Agent: acceptance, all three parts in one run, limited to what round 6 flagged and what changed since. Round 6 reports: [`a6`](acceptance-2026-11-19-a6.md), [`b6`](acceptance-2026-11-19-b6.md), [`c6`](acceptance-2026-11-19-c6.md).

## Verdict

**FAIL 1 (T-330).** T-310 is fixed: 78 of 78 timed cuts of the primary path moved input in under 1 s (378-614 ms, round 6 worst 1271 ms), and Network now reads "switched to Ethernet in 0.6 s". Every flow passes on macOS+Windows, and the smoke passes on Windows+Windows and macOS+macOS. Part A shows no change since round 6. Baseline quality and `develop` CI are green. One documentation failure is left, the same kind as T-320 in round 6:

- **T-330**: `docs/DECISIONS.md` has no T-322 line. The T-099 migration line ("counts as a change of that machine, with a new version, so the other adopts it") is not true on its own when both machines migrate at once. T-322 added an exception to "last write wins" (the moved switches go back on top as `rev + 1`, with no notice and no history entry) and an in-memory limit, and that rule exists only in `docs/tasks/T-322.md`.

## What changed since round 6

| Commit | What | Checked by |
|---|---|---|
| `2f31d4f` T-300 | Drops the unused `.sw` styles from `V3-Machine.dc.html` | A: `grep -c '\.sw'` on the mockup gives 0; `pnpm shots --check` the same as round 6 |
| `fe92474` T-322 | The start-up desk migration survives the other machine's simultaneous migration | C: tests (`network.rs` passes with the rest); docs (T-330). B: F9 on the full suite |
| `0a9a8a6` T-310 | Active path pinged every 200 ms, dropped after 2 silent 200 ms windows | B: F6 re-timing; C: DESIGN §6, DECISIONS, `HealthConfig` |
| `22674c3` | Version 0.3.0 (`Cargo.toml`, `package.json`, `tauri.conf.json`) | B: F13 (harness updated, see below); C: CI |

Nothing under `apps/tandem-ui/src` or `tools/` changed since round 6 (`git diff --stat 9fe53be 22674c3 -- apps tools mockups`: the two version fields and the T-300 mockup only).

## Part B: end-to-end flows

Round 6 harness (`docs/reports/acceptance/2026-11-19-b6/harness/`), copied to a short run directory with its paths moved to this round. Two real `tandemd` over loopback with the `acc_host` doubles, one `acc_bridge` per machine, the production UI in headless Chromium, one loopback address per path (USB4 `127.0.0.2`, Ethernet `127.0.0.3`, Wi-Fi `127.0.0.4`). Harness changes this round (in [`2026-11-19-r7/harness/`](acceptance/2026-11-19-r7/harness/)):

- `f13u.mjs`: the test `latest.json` offered `0.2.0` and the check expected the app to show `0.1.0`. After the bump to `0.3.0`, `0.2.0` is older than the app, so the app rightly said "There are no new versions" and the first full run failed those two checks (observation 1). The test now offers `0.4.0` and expects `0.3.0`.
- `smoke.sh` / `smokeall.sh`: the round 7 smoke on a fresh pair: pairing through the UI, T-291 (70 s of samples), T-280 (report on both sides, then scenes from the non-owner's window), F2 arrangement (and the engine crossing `f2g.mjs`), MCP tunnels `f5.mjs`, updates `f13u.mjs`, file-copy notice `f16b.mjs`.

The machine rebooted once in the middle of the round (WSL restart), which wiped the run directory. The F6 timings were already done and are kept from the console output in [`f6-timing.txt`](acceptance/2026-11-19-r7/f6-timing.txt). The full suite was re-run from scratch after the reboot.

### T-310 (F6 timing)

Three fresh macOS+Windows pairs (`r7a`, `r7b`, `r7c`), each with `f6repro.sh` (setup, three paths, `f6a.mjs`, `f6b.mjs`) and then `f6fail.mjs <tag> 25` (cut at a random phase against the ping, agent snapshot polled every 20 ms):

| Pair | `f6b` (official) | `f6fail` 25 cuts | ≥ 1 s |
|---|---|---|---|
| `r7a` | 521 ms | 440-614 ms | 0/25 |
| `r7b` | 486 ms | 442-610 ms | 0/25 |
| `r7c` | 541 ms | 378-600 ms | 0/25 |
| Full suite `r7mw` | 434 ms | | |

**79 of 79 cuts under 1 s, worst 614 ms** (round 6: 8 of 43 over, worst 1271 ms). This matches T-310's 600 ms worst case plus the 20 ms poll. After each official cut, Network shows "Spike when the cable was unplugged · switched to Ethernet in 0.6 s" (round 6: 1.2-1.3 s). The rest of F6 passes on each pair: the event shows in Network, the primary takes input back when the cable returns, and "Measure bandwidth" gives a value on USB4 and on Ethernet. **PASS.** The open item in T-310's Notes (`f6fail.mjs` on 3 fresh pairs) is now met.

### Full suite on macOS+Windows (`allp.sh r7mw`)

130 results: 125 PASS and 5 non-PASS, none a product failure ([`results-r7mw.jsonl`](acceptance/2026-11-19-r7/results-r7mw.jsonl), [`allp-r7mw.log`](acceptance/2026-11-19-r7/allp-r7mw.log)):

| Non-PASS | Why it is not a failure |
|---|---|
| F5 "tunnel_open with a git workspace" | My summary counted only `ok === true`; the script records the tunnel id (truthy) as `ok`, so it passed. Every F5 check passed |
| F13 "shows the versions" and "finds the new version" | Harness test version older than `0.3.0` (observation 1). Re-run with `0.4.0` on three fresh pairs: 6 of 6 F13 checks pass on each (below) |
| P10 `f16.mjs` | Known harness artifact since round 4 (polls too slowly). `f16b.mjs` passes on all three smoke pairs |
| G6 | Known harness artifact since round 4: radios already selected (`Simultaneo-2 DP`, the selected screen) and the disabled color the other machine uses |

Per flow, the passes: F1 9, F2 21 + live crossing test 2, F3 10, F4 4, F5 19, F6 6, F7 3, F8 3, F9 8, F10 2, F11 4, F12 16, F13 trouble states 8, F14 3, P6 4, G7 1, G8 1, demo sweep 1. (Two more F13 and one P10 non-PASS rows in the log came from a re-run I started on `r7mw` after its own "Leave desk" step had unpaired it; they are void and were redone on fresh pairs.)

### Smoke on three fresh pairs ([`smoke.log`](acceptance/2026-11-19-r7/smoke.log))

`r7mw2` (macOS+Windows), `r7ww` (Windows+Windows), `r7mm` (macOS+macOS), with the names "Mac"/"PC" as in round 6. **144 PASS, 0 FAIL.**

| Check | `r7mw2` | `r7ww` | `r7mm` |
|---|---|---|---|
| Pairing through the UI: same 6-digit code on both UIs, wrong code rejected, different sides | PASS | PASS | PASS |
| T-291: both stay paired for 70 s (140 samples), no "no longer trusts this one" in either log | PASS | PASS | PASS |
| T-280: both agents hold the owner's report of every shared monitor | PASS | PASS | PASS |
| T-280: scenes from the non-owner's window, 2 "Save and apply" + 2 card "Apply", each shows Changes in progress and switches the bus | PASS | PASS | PASS |
| F2: 4 sides x 3 alignments reach both machines and `tandem.toml` | PASS | PASS | PASS |
| F2: engine crosses through the new edge both ways and not through the old one (`f2g.mjs`) | in the full suite | PASS (`r7ww2`, 12 of 12) | PASS |
| F5 / P15: all MCP tunnel checks (tools, git workspace, credentials filtered, 3 processes, output, files, port, cut and recover, `bring_back`, UI live) | PASS | PASS | PASS |
| F13 / T-251: versions, new version found, "Update all" asks the other machine first, "did not come back" explained, 404 error, auto-update saved | PASS | PASS | PASS |
| P10: file copy notice with progress and "Copied" (`f16b.mjs`) | PASS | PASS | PASS |

## Part A: visual parity, text and motion

- `pnpm shots --check` (after `pnpm install` in `tools/mockup-render`): exit 0, 105 checks, every similarity and worst tile **identical** to round 6 ([`pnpm-shots.log`](acceptance/2026-11-19-r7/pnpm-shots.log)). Working tree clean afterwards. **PASS**
- Demo sweep (round 6 `harness/sweep.ts`): 279 cases, 0 console errors, 0 animations under reduced motion, 824 distinct visible lines, the same as round 6 line for line. The 10 findings are the same 10 as round 6 (nine `⌘K` with a macOS user agent on `win-win`, and the demo's macOS config path on Settings › Advanced), all accepted then ([`demo-sweep.log`](acceptance/2026-11-19-r7/demo-sweep.log)). **PASS**
- T-300: `V3-Machine.dc.html` no longer declares the `.sw` styles. **PASS**
- `motion.ts` was not re-run: no app source, style or mockup animation changed since round 6 (above), and the shots and reduced-motion sweep are identical.

## Part C: baseline quality, CI and docs

Worktree detached at `22674c3`, target dir outside the repo, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=4`, harness examples removed before the gate.

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace --no-fail-fast` | PASS: 1628 passed, 0 failed, 11 ignored (the STATUS numbers). First run, no flake (T-322's `network.rs` test included) |
| `pnpm test` | PASS: 51 files, 732 tests |
| `pnpm check` | PASS: 523 files, 0 errors, 0 warnings |
| `pnpm build`, `pnpm build:demo` | PASS |
| `develop` CI | PASS: the last 12 `ci.yml` runs on `develop` are `success`; run 44646390230 (`22674c3`) has all 11 jobs green (Linux, Windows, macOS, UI x3, mockups, screenshots, coverage, `.dmg`, Windows installer) |
| `scripts/ci/{safety,tasks,timeouts,english,protocol,status}.sh` | PASS: all `no findings` on `22674c3`, and again on this branch with T-330 and the STATUS update |

| Document | Result |
|---|---|
| DESIGN §6 vs T-310 | PASS: 200 ms active ping, 1 s backups, 2 × 200 ms windows, 600 ms worst case, 1 s with no healthy backup. The same numbers as `HealthConfig::default()` (`active_ping_interval` 200 ms, `ping_timeout` 200 ms, `misses_to_drop` 2, `worst_detection`) in `crates/tandem-link/src/manager/mod.rs`; "moved to Wi-Fi in 0.6 s" in the metrics line agrees |
| DECISIONS vs T-310 | PASS: dated T-310 line; the T-102 `took` line says "since T-310, at most ~0.6 s" |
| DECISIONS vs T-322 | **FAIL, T-330**: no T-322 line, and the T-099 migration line is not marked as refined |
| STATUS | PASS on `22674c3` (`178 done of 182`, 1628 + 732 tests). This PR adds T-330: `178 done of 183`, `T-001`…`T-330`, and T-330 in the open-task table |

## Verdict per item of `docs/ACCEPTANCE.md`

Items that round 6 passed and that no change since touches carry round 6's verdict. They were re-run here only where the suite covers them.

| Item | Verdict | Tasks | Evidence (this round) |
|---|---|---|---|
| G1 | PASS | | Unchanged UI; `english.sh` clean; sweep identical |
| G2 | PASS | | Demo sweep `win-win`/`mac-mac` identical to round 6; smoke on Windows+Windows and macOS+macOS |
| G3, G4, G5 | PASS | | Unchanged UI; shots identical |
| G6 | PASS | | Full suite button sweep: only known selected/disabled radios |
| G7, G8 | PASS | | Suite G7/G8 checks; sweep: 0 animations under reduced motion |
| G9 | PASS | | Sweep: 0 console errors, same 824 lines |
| G10 | PASS | | F9 (8 of 8) in the full suite |
| G11 | PASS | | fmt, clippy, 1628 Rust tests, 732 UI tests, check, build, CI green |
| P1-P15 | PASS | | `pnpm shots --check` identical to round 6; P15 tools list in `tandem mcp` on 4 pairs |
| F1 | PASS | | Full suite + 4 pairings in the smoke |
| F2 | PASS | | Full suite; engine crossing on Windows+Windows and macOS+macOS |
| F3 | PASS | | Full suite; scenes from the non-owner on 3 pairs (12 of 12) |
| F4 | PASS | | Full suite; owner's report on both sides on 3 pairs |
| F5 | PASS | | Full suite + 3 smoke pairs |
| F6 | PASS | | 79 of 79 cuts under 1 s (T-310 fixed) |
| F7, F8 | PASS | | Full suite; `f16b.mjs` on 3 pairs |
| F9 | PASS | | Full suite (also after T-322) |
| F10, F11, F12, F14 | PASS | | Full suite |
| F13 | PASS | | 6 of 6 on 3 fresh pairs with a `latest.json` newer than 0.3.0 |
| Docs consistency (part C) | **FAIL** | T-330 | No DECISIONS line for T-322 |
| H1-H5 `[hw]` | Pending hardware | | Not part of this run; last real run in `docs/reports/hw-2026-11-18-cable.md`, the rest stays in each task's pending hardware list |

## Observations (not failures)

1. **F13 harness out of date after the release bump.** `f13u.mjs` hard-coded `0.1.0` and `0.2.0`. Once the app is `0.3.0`, the app answering "There are no new versions" to a `0.2.0` `latest.json` is correct. Fixed in the harness copy under `2026-11-19-r7/harness/`.
2. **The run directory was lost in a machine reboot** halfway through the round. The F6 raw `results.jsonl` went with it, so its numbers come from the console output (`f6-timing.txt`). Everything after the reboot (full suite, smoke, part A, part C) ran from scratch.
3. The F5 Tunnels screenshots were removed, as in earlier rounds, because they show the test project's path.

## Fix tasks

| Task | Failure | Item |
|---|---|---|
| [T-330](../tasks/T-330.md) | `docs/DECISIONS.md` has no T-322 line; the T-099 migration line does not tell the whole rule | Docs consistency (part C) |
