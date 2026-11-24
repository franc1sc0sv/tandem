# Tandem v0.3 acceptance, part C, round 5: baseline quality, docs, CI, G items

Date: 2026-11-19 · base: `origin/develop` 95af721 (T-244, #176) · branch `qa/acceptance-2026-11-19-c5`.
Scope: `cargo fmt/clippy/test`, `pnpm test/check/build`, `develop` CI, `scripts/ci/*.sh`, documents against code, and the items of `docs/ACCEPTANCE.md` that concern docs, release and installers. Previous round: `docs/reports/acceptance-2026-11-19-c4.md` (failures T-231, T-260, T-261).

**Verdict of part C: FAIL (1 open failure: new T-290, `docs/STATUS.md` has drifted again). The three failures of round 4 (T-231, T-260, T-261) are verified fixed.**

## Previous round, verified

| Task | Result | Evidence |
|---|---|---|
| T-231 (STATUS.md stale) | Fixed, then drifted again | It was correct at 3f12397; `status.sh` fails again at 95af721 because T-243, T-244, T-245, T-250 and T-251 were added since (new T-290) |
| T-260 (document protocol v41) | PASS | `docs/DESIGN.md` §4.5 states `PROTOCOL_VERSION = 41`, `CAPABILITIES_SINCE = 10`, `SIDES_SINCE = 16`, the "same version only" rule, the independent secure-desktop pipe version (1) and history pointer; the values equal `crates/tandem-proto/src/lib.rs`, "last bumped by T-240" is true (`git log -S` finds 8504769), and `scripts/ci/protocol.sh` (run in `ci.yml`) passes |
| T-261 (crate list, T-027) | PASS | `ls crates` gives 16 crates and all 16 are in DESIGN §4.2 with a role, plus `tools/windows/installer`; DESIGN lines 160 and 241 say T-027 is implemented with hardware verification pending; DECISIONS has the dated T-261 line superseding A7's wording |

## Baseline quality (run in this worktree, build dir outside the repo)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace` | PASS, run twice: 1613 passed, 0 failed, 11 ignored (`hardware` and latency benches) both times |
| `pnpm install --frozen-lockfile`, `pnpm test` | PASS: 51 files, 733 tests |
| `pnpm check` (svelte-check, `--fail-on-warnings`) | PASS: 523 files, 0 errors, 0 warnings |
| `pnpm build` | PASS |
| `scripts/release/version.sh`, `scripts/release/test.sh` | PASS (`all tests passed`); version is `0.1.0`, bumped when the release is cut (`docs/dev/release.md`, step 1), not a failure |

## `develop` CI

The last 15 runs of `ci.yml` on `develop` (71ed1ca to 95af721): 14 `success`, 1 `failure`. The failure is run 44520508090 (92a1dc1, a docs-only report merge): job `ui (mockups)` failed at the step `playwright install --with-deps chromium` (a download/infrastructure step, not a test); the next run (fcdaf1e) and all later ones are green. No test failed. Run 44533874525 (95af721, the base of this round) has all 11 jobs green: `ubuntu-latest`, `windows-latest`, `macos-latest`, `ui` on the three OSes, `ui (mockups)`, `ui (screenshots)`, `coverage`, `dmg (macos-latest)` and `installer (windows)`. No `pairing.rs` or `drag.rs` timeout in these runs. No open PR at the time of the check.

Note: T-190 (pairing flake on macOS) is still `blocked` without a reproduction and has not failed again. Not counted as a new failure.

## Consistency scripts (`scripts/ci/`)

| Script | Result |
|---|---|
| `safety.sh` | PASS (`no findings`) |
| `tasks.sh` | PASS |
| `timeouts.sh` | PASS |
| `english.sh` | PASS |
| `protocol.sh` | PASS |
| `status.sh` | FAIL, T-290: header must say `168 done of 173` (`T-001`…`T-261`); `docs/STATUS.md` says `163 done of 168` |
| `privacy.sh` | PARTIAL: `PRIVACY_PATTERNS` is not set in this environment, so only the 192.168.x.x check ran (`no findings`); the secret-pattern check was skipped. A manual grep for the owner's name, e-mail, home paths and Windows profile paths in tracked files outside `docs/reports/` found only the public repo handle (repo URLs, bundle ids `com.franc1sc0sv.tandem.*`, `.claude/agents`) and made-up test data |

`docs/STATUS.md` (T-290) is stale in four places: the header counts (163 of 168, should be 168 of 173; 169 of 174 after T-290 closes), the test row (1608 Rust + 682 UI, now 1613 + 733), the "Open tasks" row "T-231 | in this PR" (T-231 is `done`) and the "status as of ... (after T-261)" note. Per the decision in T-231, `status.sh` is release-only, so this is caught here and not in `ci.yml`.

## Task files

173 task files on `develop` (174 with T-290): 168 `done`, 4 `blocked` (T-088, T-095, T-096, T-190), 1 `todo` (T-124). Frontmatter is valid on all of them (`tasks.sh`). T-243, T-244, T-245, T-250 and T-251 are `done` and merged. T-124 (end-to-end acceptance) is this process itself.

## Documents against code

| Item | Result |
|---|---|
| `docs/GLOSSARY.md` terms match the UI and code | PASS (unchanged since round 4; T-243..T-251 added no new terms) |
| `CLAUDE.md` is English and describes the real protocol (`develop`, `task/T-<id>`, quality gate) | PASS |
| `docs/DESIGN.md` §4.2 crate list (16 crates) and §4.5 protocol v41 | PASS (T-261, T-260) |
| T-027 state in DESIGN/DECISIONS | PASS (T-261) |
| `docs/DECISIONS.md` has dated lines for T-243 and T-245 (the tasks that took a decision) | PASS |
| `docs/dev/install-windows.md` and `release.md` match the `release.yml` jobs and scripts | PASS (`scripts/release/test.sh`, `dmg` and `installer (windows)` jobs green) |
| `docs/STATUS.md` | FAIL, T-290 |

## `docs/ACCEPTANCE.md` items in this part

| Item | Result | Evidence |
|---|---|---|
| G1 | PASS | no third-machine flow in the UI sources; `english.sh` clean (visual check belongs to part A) |
| G9 (English only, no `undefined`/`{{` in sources) | PASS for the text rule | `english.sh` no findings; console and visible-text checks belong to part A/B |
| G11 | PASS | fmt, clippy, 1613 Rust tests, 733 UI tests, check, build, CI green on Linux, Windows and macOS |
| F13 updates | PASS | `scripts/release/test.sh` (`latest-json`), update tests in the suites |
| Installers (`dmg`, Windows installer) | PASS | CI jobs `dmg (macos-latest)` and `installer (windows)` green on 95af721 |
| H1-H5 `[hw]` | Not part C | Last real run: `docs/reports/hw-2026-11-18-cable.md`; whoever merges the parts should read the hardware lists |

G11 passes on its own terms (every listed gate is green); the STATUS drift is a documentation failure counted under "Documents against code".

## Fix tasks

- T-290 (new): refresh `docs/STATUS.md` (counts, test numbers, open-task table, date note) until `scripts/ci/status.sh` passes.
