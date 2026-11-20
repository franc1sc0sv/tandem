# Tandem v0.3 acceptance, round 8: confirmation run

Date: 2026-11-19. Base: `origin/develop` at `6356a95` (T-330, #195). Branch `qa/acceptance-2026-11-19-r8`. Agent: acceptance. Scope: confirm that round 7's only failure (T-330) is fixed and that nothing else moved. Round 7 report: [`acceptance-2026-11-19-r7.md`](acceptance-2026-11-19-r7.md).

## Verdict

**FINAL_OK.** T-330 is fixed, the six CI scripts report no findings, `develop` CI is green on `6356a95`, and the `v0.3.0` release is complete. Nothing that round 7 verified has changed, so its evidence stands for every functional, visual and baseline item.

## Did code change since round 7?

Claim checked: `git diff 22674c3..6356a95 --name-only` lists only files under `docs/`. Nothing under `crates/`, `apps/`, `tools/`, `mockups/`, `scripts/`, `.github/` or any manifest changed. The two commits are `95d2426` (round 7 report, harness, screenshots, T-330) and `6356a95` (T-330 fix). The docs files touched: `docs/DECISIONS.md`, `docs/DESIGN.md`, `docs/STATUS.md`, `docs/tasks/T-330.md`, `docs/reports/acceptance-2026-11-19-r7.md` and `docs/reports/acceptance/2026-11-19-r7/*`. The claim holds, so no rebuild and no flow re-run was needed.

## T-330

| Check | Result |
|---|---|
| `docs/DECISIONS.md` has a dated T-322 line | PASS. Line 160, `2026-11-19 · T-322 · Exception to "last write wins" for a simultaneous migration sibling ...`: names the cause (both agents bump the same `rev`), the rule (own moved switches go back on top as `rev + 1`, no history entry, no "Changed from ..."), when the memory is dropped, and the in-RAM limit |
| T-099 line marked refined | PASS. Line 85 starts `(Refined by T-322.)` |
| `docs/DESIGN.md` mentions the exception | PASS. Line 114, "**Last write wins** (D5), except right after both machines migrate an old desk at once ... (DECISIONS T-322)" |
| Text matches the code | PASS (spot check). `crates/tandemd/src/agent.rs` has the migration-moved record (line 166), the "goes back on top" step (line 856) and the test `a_moved_setting_survives_the_other_machines_simultaneous_migration` (line 2261) |
| `docs/STATUS.md` consistent | PASS. 179 done of 183, `T-001`...`T-330`; `scripts/ci/status.sh` agrees |

## Gate scripts on `6356a95`

Run from a clean worktree at `6356a95`.

| Script | Result |
|---|---|
| `scripts/ci/safety.sh` | no findings |
| `scripts/ci/tasks.sh` | no findings |
| `scripts/ci/timeouts.sh` | no findings |
| `scripts/ci/english.sh` | no findings |
| `scripts/ci/protocol.sh` | no findings |
| `scripts/ci/status.sh` | no findings |

## develop CI

`gh run list --branch develop`: run `44661878005` on `6356a95` finished **success** on all 11 jobs (ubuntu, windows, macos, ui x3 + screenshots + mockups, coverage, dmg, Windows installer). The runs before it (`95d2426`, `22674c3`) were also green.

## Release v0.3.0

`gh release view v0.3.0`: published, not a draft, not a pre-release, tag target `22674c3`. Assets:

| Asset | Size |
|---|---|
| `Tandem_0.3.0_aarch64.dmg` | 12113563 |
| `Tandem_0.3.0_x64-setup.exe` | 8655410 |
| `Tandem_0.3.0_aarch64.app.tar.gz` + `.sig` (macOS updater package) | 12438851 / 424 |
| `Tandem_0.3.0_x64-setup.exe.sig` (Windows updater package signature) | 436 |
| `SHA256SUMS.txt` | 282 |
| `latest.json` | 17680 |

I downloaded the dmg, the setup.exe and the app tarball and ran `sha256sum -c SHA256SUMS.txt`: all three **OK**. `latest.json` says version `0.3.0` and lists `darwin-aarch64` (the `.app.tar.gz`) and `windows-x86_64` (the `x64-setup.exe`).

## Items not re-run, with round 7 evidence

All of these depend on code, UI and mockup files that are byte-identical between `22674c3` and `6356a95`.

| Area | Evidence |
|---|---|
| Part A, screens and mockup parity (similarity, worstTile, animations, reduced motion) | Round 7: no change since round 6, `pnpm shots --check` the same; T-300 mockup `grep -c '\.sw'` gives 0 |
| Part B, flows F1-F14, P6, G7, G8, demo sweep | Round 7: 130 results on macOS+Windows, 125 PASS and 5 non-PASS that are harness artifacts or void rows, re-verified on fresh pairs ([`results-r7mw.jsonl`](acceptance/2026-11-19-r7/results-r7mw.jsonl)) |
| F6 / T-310 (failover under 1 s) | Round 7: 79 of 79 cuts under 1 s, worst 614 ms ([`f6-timing.txt`](acceptance/2026-11-19-r7/f6-timing.txt)) |
| F9 / T-322 behavior | Round 7: F9 8 PASS on the full suite; unit test above |
| Smoke on three fresh pairs | Round 7: 144 PASS, 0 FAIL ([`smoke.log`](acceptance/2026-11-19-r7/smoke.log)) |
| Baseline (`fmt`, `clippy -D warnings`, `cargo test`, `pnpm test`, `pnpm check`, `pnpm build`) | Round 7: green. Source unchanged; CI on `6356a95` ran the same gate and passed |
| `[hw]` items | As in round 7: covered by the hardware suite (T-123) or listed under their task's `## Pending hardware verification` |

## Tasks created

None.

ACCEPTANCE: FINAL_OK
