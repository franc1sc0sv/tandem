# Tandem v0.3 acceptance, part C, round 4: baseline quality, docs, CI, G items

Date: 2026-11-19 · base: `origin/develop` 8504769 (T-240, #169) · branch `qa/acceptance-2026-11-19-c4`.
Scope: `cargo fmt/clippy/test`, `pnpm test/check/build`, `develop` CI, `scripts/ci/*.sh`, documents against code, and the items of `docs/ACCEPTANCE.md` that concern docs, release and installers. Previous round: `docs/reports/acceptance-2026-11-18-c3.md`.

**Verdict of part C: FAIL (3 open failures: T-231 existing and still open, new T-260 and T-261).**

## Baseline quality (run in this worktree, build dir outside the repo)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace` | PASS, run twice: 1608 passed, 0 failed, 11 ignored (`hardware` and latency benches) |
| `pnpm install --frozen-lockfile`, `pnpm test` | PASS: 48 files, 682 tests |
| `pnpm check` (svelte-check, `--fail-on-warnings`) | PASS: 519 files, 0 errors, 0 warnings |
| `pnpm build` | PASS |
| `scripts/release/version.sh`, `scripts/release/test.sh` | PASS (`all tests passed`); version is `0.1.0` in all three files, see "Release" |

## `develop` CI

The last 15 runs of `ci.yml` on `develop` (from 0658d44 to 8504769) are all `success`. Run 44511522807 (8504769) has all 11 jobs green: `ubuntu-latest`, `windows-latest`, `macos-latest`, `ui` on the three OSes, `ui (mockups)`, `ui (screenshots)`, `coverage`, `dmg (macos-latest)` and `installer (windows)`. No `pairing.rs` or `drag.rs` timeout in these 15 runs; T-230, T-235 and T-195 hold. No PR is red or pending except #170 (T-242, open, not part of `develop`).

Note: T-190 (pairing flake on macOS) is still `blocked` for lack of a reproduction; its unmet criteria remain unmet and it has not failed again. It is not counted as a new failure.

## Consistency scripts (`scripts/ci/`)

| Script | Result |
|---|---|
| `safety.sh` | PASS (`no findings`) |
| `tasks.sh` | PASS |
| `timeouts.sh` | PASS (T-234 holds; all 5 jobs of `release.yml` and all of `ci.yml` have `timeout-minutes`) |
| `english.sh` | PASS (T-210 holds; the remaining accents are the allow-listed test data in `english-allow.txt`) |
| `status.sh` | FAIL, T-231 (still `todo`, being fixed in parallel): header must say `159 done of 165 (T-001…T-241)` and T-231 is missing from "Open tasks" |
| `privacy.sh` | SKIPPED: `PRIVACY_PATTERNS` is not set in this environment. Manual grep for the owner's name, e-mail and home paths in tracked files outside `docs/reports/` found none; it did find the Windows account `PC` in `docs/dev/hardware.md` and `docs/tasks/T-024.md` (a Windows user-profile path under the generic account `PC`), a generic account name, left as a note |

`docs/STATUS.md` (T-231) is also stale in more than the header: it says 127 done of 141, "1442 Rust + 619 UI" tests (now 1608 and 682), lists T-195 and T-201…T-208 as open (all `done`), T-027 as `todo` (now `done`), and its CI line still names T-160/T-190 as known flaky tests. The count in the "Open tasks" table must be taken when T-231's PR is rebased, since this PR adds T-260 and T-261 (two more tasks).

## Task files

165 task files: 159 `done`, 4 `blocked` (T-088, T-095, T-096, T-190), 2 `todo` (T-124, T-231). Frontmatter is valid on all of them; no `done` task leaves a criterion unchecked outside the hardware and notes sections (`tasks.sh`). Round 3 fixes T-230, T-232, T-233, T-234, T-235 and the part B and A fixes up to T-241 are `done` and merged. T-124 (end-to-end acceptance) is this process itself.

## Documents against code

| Item | Result |
|---|---|
| `docs/GLOSSARY.md` terms match the UI and code (machine, peer, scene, tunnel, link, setup/`wizard`) | PASS |
| `CLAUDE.md` is English and describes the real protocol (`develop`, `task/T-<id>`, quality gate) | PASS |
| Protocol version 41 documented | FAIL, T-260: `PROTOCOL_VERSION: u16 = 41` exists only in `crates/tandem-proto/src/lib.rs` and the T-240 commit message; no document gives the number, the rule or the `*_SINCE` constants |
| `docs/DESIGN.md` §4.2 crate list | FAIL, T-261: 13 crates listed, 16 exist (`tandem-cli`, `tandem-workspace`, `tandem-hw` missing) |
| T-027 state in DESIGN/DECISIONS | FAIL, T-261: T-027 is `done` (#166, `docs/dev/secure-desktop.md`) but DESIGN lines 147 and 228 say "needs the user" and DECISIONS A7 says "T-027 waits for the user" |
| `docs/dev/install-windows.md` and `release.md` match `release.yml` jobs, scripts and files (`scripts/release/*`, `tauri.updater.json`, `tools/windows/installer/*`) | PASS |
| `docs/STATUS.md` | FAIL, T-231 (see above) |

## `docs/ACCEPTANCE.md` items in this part

| Item | Result | Evidence |
|---|---|---|
| G1 | PASS | `grep` for third-machine flows finds none in the UI sources; `english.sh` clean; (visual check belongs to part A) |
| G9 (English only, no `undefined`/`{{` in sources) | PASS for the text rule | `english.sh` no findings; console and visible-text checks belong to part A/B |
| G11 | PASS | fmt, clippy, 1608 Rust tests, 682 UI tests, check, build, CI green on Linux, Windows and macOS |
| F13 updates: version lookup against a test `latest.json`, plan for both machines | PASS | `scripts/release/test.sh` (`latest-json`), UI/agent update tests in the 1608; real release documented in `docs/dev/release.md` (draft rc flow, updater key, signing, pending items) |
| Installers (`dmg`, Windows installer with install/reinstall/uninstall test) | PASS | CI jobs `dmg (macos-latest)` and `installer (windows)` green on 8504769 |
| H1-H5 `[hw]` | Not part C. Last real run: `docs/reports/hw-2026-11-18-cable.md` (H1, H2 pass; H4 failed because the Mac's Accessibility permission was not granted, which needs the user; T-196, T-197, T-237 merged since). H5 is documented (DECISIONS A6, `docs/reports/hw-2026-11-18-cable.md`) | Whoever merges the parts should read the hardware lists, not this report |

### Release

Version is `0.1.0` in `Cargo.toml`, `tauri.conf.json` and `package.json` while the docs and the `ACCEPTANCE.md` title speak of v0.3. `docs/dev/release.md` ("Cutting a release", step 1) makes the bump part of cutting the release, so this is not a failure; it must happen before the tag.

## Fix tasks

- T-231 (existing, `todo`): STATUS.md stale; `status.sh` fails.
- T-260 (new): document `PROTOCOL_VERSION` 41, the rule, and a CI check on the number.
- T-261 (new): DESIGN §4.2 crate list and T-027 status in DESIGN/DECISIONS.
