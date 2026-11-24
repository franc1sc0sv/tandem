# Tandem v0.3 acceptance, part C, round 6 (final): baseline quality, docs, CI, G items

Date: 2026-11-19 · base: `origin/develop` 9fe53be (T-291, #185) · branch `qa/acceptance-2026-11-19-c6`.
Scope: `cargo fmt/clippy/test`, `pnpm test/check/build`, `develop` CI, `scripts/ci/*.sh`, documents against code, and the items of `docs/ACCEPTANCE.md` that concern docs, release and installers. Previous round: `docs/reports/acceptance-2026-11-19-c5.md` (failure T-290).

**Verdict of part C: FAIL, 3 open failures: T-320 (DECISIONS has no T-291 entry), T-321 (stray root `package.json`), T-322 (`network.rs` test flake under load). T-290 is verified fixed.**

## Previous round, verified

| Task | Result | Evidence |
|---|---|---|
| T-290 (STATUS.md stale) | PASS | `scripts/ci/status.sh` prints `no findings` at 9fe53be; the header says `173 done of 177` (`T-001`…`T-291`), the test row says 1623 Rust + 732 UI, which are the numbers measured in this run, and the open-task table is right |
| T-190 closed as superseded by T-291 | PASS | `docs/tasks/T-190.md` is `status: done`, criteria checked, and its Notes end with a dated "superseded by T-291" update that names the failing run (44576241294) and the cause; `docs/STATUS.md` says the flake "is explained and fixed by T-291" |

## Baseline quality (worktree detached at 9fe53be, build dir outside the repo, `CARGO_INCREMENTAL=0`)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace` | FAIL on the first run, PASS on the second. First run (machine load average 17 to 23 on 8 CPUs, other acceptance parts building): `a_local_setting_moved_to_the_desk_does_not_leave_the_machines_apart` timed out (T-322). Second run (`--no-fail-fast`, load 5): 1623 passed, 0 failed, 11 ignored |
| `pnpm install --frozen-lockfile`, `pnpm test` | PASS: 51 files, 732 tests |
| `pnpm check` (svelte-check, `--fail-on-warnings`) | PASS: 523 files, 0 errors, 0 warnings |
| `pnpm build` | PASS |

The flake, in detail (T-322): `crates/tandemd/tests/network.rs:1003`, `within(on_a.wait_for(off))`, 20 s limit; the other tests of the binary take 1 to 2 s, so A never received B's migrated clipboard switch. Running that test alone 15 times under the same load failed 1 of 15 with the same panic; 14 passed. T-235 had fixed another step of the same test. CI has not shown it (14 of the last 15 `develop` runs green), but a gate that fails on a loaded machine in two of 17 runs is a failure under G11, the same standard T-235 applied.

## `develop` CI

The last 15 runs of `ci.yml` on `develop` (3f12397 to 9fe53be): 14 `success`, 1 `failure`. The failure is run 44520508090 (92a1dc1, a docs-only report merge), already described in round 5: `ui (mockups)` failed at `playwright install --with-deps chromium` (a download step); the next run and all later ones are green. The run of this round's base, 44590376464 (9fe53be), has all 11 jobs green: `ubuntu-latest`, `windows-latest`, `macos-latest`, `ui` on the three OSes, `ui (mockups)`, `ui (screenshots)`, `coverage`, `dmg (macos-latest)` and `installer (windows)`. T-291's own history shows one macOS failure on PR #184 (`pairing_hot_opens_the_input_session`, run 44576241294), which is the cause T-291 fixed; no `pairing.rs` or `drag.rs` timeout in the runs after it. No open PR at the time of the check.

## Consistency scripts (`scripts/ci/`)

| Script | Result |
|---|---|
| `safety.sh` | PASS (`no findings`) |
| `tasks.sh` | PASS |
| `timeouts.sh` | PASS |
| `english.sh` | PASS |
| `protocol.sh` | PASS |
| `status.sh` | PASS at 9fe53be. This PR adds T-320..T-322, so it also updates the STATUS header (`173 done of 180`, `T-001`…`T-322`) and the open-task table, and `status.sh` passes on the branch |
| `privacy.sh` | PARTIAL: `PRIVACY_PATTERNS` is not set in this environment, so only the 192.168.x.x check ran (`no findings`); the secret-pattern check was skipped |

## Task files

177 task files on `develop` (180 with T-320..T-322): 173 `done`, 3 `blocked` (T-088, T-095, T-096), 1 `todo` (T-124, this process). T-190 is no longer `blocked`. Frontmatter valid on all (`tasks.sh`).

## Documents against code

| Item | Result |
|---|---|
| `docs/DESIGN.md` §4.5 protocol v41 | PASS: `PROTOCOL_VERSION = 41`, `CAPABILITIES_SINCE = 10`, `SIDES_SINCE = 16` equal `crates/tandem-proto/src/lib.rs` (and the test that pins them); "last bumped by T-240" is true (`git log -S` finds 8504769, no later bump); `protocol.sh` passes. T-280 and T-291 did not change the wire |
| `docs/DESIGN.md` §4.2 crate list, T-027 state | PASS (unchanged since round 5) |
| `docs/STATUS.md` | PASS (T-290) |
| `docs/GLOSSARY.md` | PASS: T-270, T-280, T-290 and T-291 add no user-visible term; the removed "Allow remote execution" row appears nowhere in the docs except as superseded in DECISIONS |
| Task frontmatter (`status`, `depends_on`, `needs`) | PASS (`tasks.sh`, and the counts above are recomputed from the files) |
| `docs/DECISIONS.md` T-270 | PASS: dated line (user decision), and T-115's line is marked "(superseded by T-270)" |
| `docs/DECISIONS.md` T-280 | PASS: dated line (a machine keeps the last report of each machine; each `ControlMsg::Monitors` replaces the sender's list; the owner re-sends when channels change) |
| `docs/DECISIONS.md` T-291 | FAIL, T-320: no T-291 line (`grep "T-291" docs/DECISIONS.md` is empty), and the T-152 line (a rejection for an untrusted key unpairs) is no longer true as written: since T-291 a peer just paired here is forgotten only once the other agent accepted this side once, or after `PAIRING_SETTLE` (60 s). The reasoning exists only in `docs/tasks/T-291.md` |
| Repository hygiene | FAIL, T-321: `package.json` (name `agent-a0781272cae4a878b`, `npm init -y` output) and a 9-line `pnpm-lock.yaml` with an empty importer are tracked at the repository root since T-119; nothing refers to them (CI and release use `apps/tandem-ui` and `tools/mockup-render`) |

## `docs/ACCEPTANCE.md` items in this part

| Item | Result | Evidence |
|---|---|---|
| G1 | PASS | no third-machine flow in the UI sources; `english.sh` clean (visual check belongs to part A) |
| G9 (English only, no `undefined`/`{{` in sources) | PASS for the text rule | `english.sh` no findings; console and visible text belong to parts A and B |
| G11 | FAIL | fmt, clippy, UI tests, check, build and CI are green; `cargo test --workspace` failed on its first run under load (T-322) |
| F13 updates | PASS | unchanged since round 5 (`scripts/release/test.sh` and the update tests are in the suites that ran) |
| Installers (`dmg`, Windows installer) | PASS | CI jobs `dmg (macos-latest)` and `installer (windows)` green on 9fe53be |
| H1-H5 `[hw]` | Not part C | Last real run: `docs/reports/hw-2026-11-18-cable.md`; whoever merges the parts reads the hardware lists |

## Fix tasks

- T-320: record T-291 in DECISIONS and mark the T-152 rejection line as refined by it; check that no document says a rejection always unpairs.
- T-321: remove the stray root `package.json` and `pnpm-lock.yaml`.
- T-322: find and fix the lost desk update that makes `a_local_setting_moved_to_the_desk_does_not_leave_the_machines_apart` time out under load, with a deterministic test.
