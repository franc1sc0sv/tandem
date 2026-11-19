# Tandem v0.3 acceptance, part C, round 3: CI health, consistency scripts, English-only

Date: 2026-11-19 · base: `origin/develop` bbdc245 (T-202) · branch `acceptance3-c`.
Scope: round 2 part C failures (T-190 to T-193), `develop` CI health on three OSes (including T-195), `scripts/ci/*.sh`, and the English-only rule. T-210 (English CI guard) runs in parallel and is not duplicated; remaining Spanish is only listed. Not repeated this round: local `cargo test`/`pnpm` (CI on `develop` is the verifier).

**Verdict of part C: FAIL (6 failures, tasks T-230 to T-235).**

## Round 2 failures

| Task | Result |
|---|---|
| T-190 pairing flake on macOS | FAIL: still `blocked`, cause unknown (waits are labeled). The same class of timeout now hits `drag.rs` (T-230) |
| T-191 CI timeouts | PARTIAL: all 7 jobs of `ci.yml` have `timeout-minutes` (PASS); `release.yml` jobs `artifacts` and `release` have none (T-234) |
| T-192 STATUS.md | FAIL: `scripts/ci/status.sh` exits 1: expects "138 done of 145 (`T-001`…`T-209`)", file says "127 done of 141 (…`T-208`)" (T-231) |
| T-193 task states | PARTIAL: `tasks.sh` passes and T-161/T-150 are fixed, but `T-205.md` is `todo` although PR #143 merged, with its criterion unmet (T-232) |

## CI health on `develop`

| Run | Result |
|---|---|
| T-195 (f31ae1e), T-196, T-209, T-201, T-204, T-203, T-205, T-207, T-208, T-200 | success on all 11 jobs |
| T-197 (559ff83, run 44435661690, after T-195) | FAIL on `ubuntu-latest`: `canceling_midway_deletes_what_was_copied_on_the_destination`, `drag.rs:34`, 20 s timeout (T-230) |
| T-198 (run 44427831943, before T-195) | FAIL on `macos-latest`: `the_drag_copies_to_the_destination_and_returns_how_it_ended`, `drag.rs:32` (T-230) |
| T-202 (bbdc245, run 44438617523) | in progress when checked |

T-195 fixed the pairing/modules timings (no timeout seen in those files since), but not the `drag.rs` waits, whose `within` has no label.

Local run (commit gate, full `cargo test --workspace` on `develop` bbdc245): `network::a_local_setting_moved_to_the_desk_does_not_leave_the_machines_apart` failed once (`network.rs:828`, `None` instead of `Some(false)`) and passes 6 of 6 in isolation: a race in the test (T-235).

## Consistency scripts

`privacy.sh`, `safety.sh`, `tasks.sh`: PASS. `status.sh`: FAIL (T-231). `status.sh` only runs in `release.yml`.

## English-only (report only; T-210 owns the guard)

Remaining Spanish prose: `apps/tandem-ui/vite.config.ts` comments, `mockups/main.html` and `next.html` comments, `docs/reports/acceptance/2026-11-18-b/harness/acc_bridge.rs:294`, `docs/tasks/T-191.md` notes (T-232); `docs/examples/*.toml` and `docs/examples/v0.2/*.toml` comments, and about 27 Spanish-named files in `docs/screenshots/` (T-233). Legitimate Spanish data left alone: keyboard layout labels in `keymap.rs`, accented test fixtures, Spanish `netsh` words in `tandem-link`, legacy ids in `tandem-core/src/legacy.rs` and `tandem-sim` tests, and the GLOSSARY table.

## Fix tasks

T-230 (drag.rs / T-190 flake), T-231 (STATUS.md), T-232 (T-205 state and Spanish in code and mockups), T-233 (examples and screenshot names), T-234 (release.yml timeouts), T-235 (network.rs race).
