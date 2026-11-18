# Tandem v0.3 acceptance, part C, round 2: baseline quality, decisions and backlog

Date: 2026-11-18 · base: `origin/develop` b3841bc (T-152, T-155, T-159) · branch `qa/acceptance-2026-11-18-c2`.
Scope: same as round 1 (`acceptance-2026-11-18-c.md`): baseline quality, non-visual G1-G11, documentation consistency, plan v0.3, hardware and security.

**Verdict of part C: FAIL (4 new failures, tasks T-190 to T-193).**

## The 6 failures from round 1

| Task | Result |
|---|---|
| T-160 flaky tests | PARTIAL: `session.rs` and the MCP e2e did not fail again, but `pairing_hot_opens_the_input_session` failed again on macOS in `develop` after the fix (run 44368941401, 97442ab). Moves to T-190 |
| T-161 "Add machine" | FIXED in the code (T-142: "Pair the other machine", `pair-peer`); the file `T-161.md` is still `todo` (T-193) |
| T-162 personal data | FIXED: there is no longer an IP or Mac user in the tree; `scripts/ci/privacy.sh` runs in CI and passes |
| T-163 STATUS, TESTING, hardware | PARTIAL: `TESTING.md` and `dev/hardware.md` up to date; `STATUS.md` is old again (T-192) |
| T-164 unchecked criteria | FIXED for the cited tasks; T-150 remains (T-193) |
| T-165 SAFETY comments | FIXED: `scripts/ci/safety.sh` passes and runs in CI |

## 1. Baseline quality (G11)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace`, run 1 | PASS: 1465 tests, 0 failures, 11 ignored |
| `cargo test --workspace`, run 2 | PASS: same |
| `cargo test -p tandemd --test pairing` x6 | PASS 6 of 6 |
| `pnpm test` | PASS: 47 files, 667 tests |
| `pnpm check` | PASS: 518 files, 0 errors, 0 warnings |
| `pnpm build` | PASS |
| `scripts/ci/privacy.sh`, `scripts/ci/safety.sh` | PASS |
| `develop` CI at b3841bc (run 44372328096) | PASS on all jobs |
| `develop` CI, history since T-160 | FAIL (T-190, T-191): 97442ab red on macOS (`pairing_hot_opens_the_input_session`, 30 s timeout); e86b9c8 red because of a UI test (`acceptance-b.test.ts`, T-158) after merging two PRs, fixed by the next merge; run 44345845261 (4751832) has the `ubuntu-latest` job hung in `cargo test` for more than 2 h 20 min and `ci.yml` only has `timeout-minutes` on one job |

## 2. Non-visual rules G1-G11 and documentation consistency

| Rule | Result |
|---|---|
| G1 two machines | PASS: no "Add machine" or "Add another machine" in `apps/` or `crates/` outside comments and the documented masks |
| G2 nothing fixed to Mac/PC | PASS: `Machine::{A,B}`; no `Machine::Mac/Pc` or `set_mode` in docs or code |
| G5 no filler | PASS: no "Keyboard here" or "requires cable" |
| G7, G8, G3, G4, G6, G9 | visual or with UI: parts A and B |
| G10 | covered in the agent by `tandemd/tests/network.rs`; the UI part belongs to part B |
| G11 | FAIL because of T-190 and T-191 |

Documents: `DECISIONS`, `DESIGN` and `ACCEPTANCE` consistent. FAIL T-192: `STATUS.md` says T-140…T-147 and T-160 are `todo` (they are `done`), "92 done of 111" (there are 115 `done` of 121 files). FAIL T-193: T-161 `todo` although it is fixed; T-150 `done` with an unchecked criterion that is not hardware.

## 3. Plan v0.3 (T-093 to T-131)

No change from round 1: 36 of 39 `done`; T-095 and T-096 `blocked` (replaced), T-124 `todo` (this acceptance). The "Pending hardware verification" lists are concrete.

## 4. Hardware

H1 to H5: pending hardware, each with its concrete check (T-123, T-093, T-094, T-104, T-108, T-109, T-110); they do not count as failures. The T-123 suite has not yet run on the real machines.

## 5. Security

Secrets: none versioned. User data: PASS (T-162). `unsafe` with `// SAFETY:`: PASS (`safety.sh`). Copied GPL code: no findings.

## Fix tasks

| Task | Failure |
|---|---|
| [T-190](../tasks/T-190.md) | `pairing_hot_opens_the_input_session` still fails on macOS in CI |
| [T-191](../tasks/T-191.md) | CI without `timeout-minutes`; Ubuntu job hung for more than 2 h |
| [T-192](../tasks/T-192.md) | `STATUS.md` out of date |
| [T-193](../tasks/T-193.md) | T-161 `todo` and T-150 `done` with an unchecked criterion |

`ACCEPTANCE: FAIL`
