# Tandem v0.3 acceptance, part C: baseline quality, decisions and backlog

Date: 2026-11-18 · base: `origin/develop` 04f2967 (T-131) · detached worktree `acceptance-c` · branch `qa/acceptance-2026-11-18-c`.
Scope: baseline quality, non-visual rules G1-G11, documentation consistency, plan v0.3 (T-093 to T-131), hardware items and security. Visual parity (part A) and the flows with UI (part B) were not touched here.

**Verdict of part C: FAIL (6 failures, tasks T-160 to T-165).**

## 1. Baseline quality (G11)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace`, run 1 | PASS: 1442 tests, 0 failures, 11 ignored |
| `cargo test --workspace`, run 2 | PASS: 1442 tests, 0 failures, 11 ignored |
| Flaky tests locally | None between the two runs |
| `pnpm test` (vitest) | PASS: 46 files, 619 tests |
| `pnpm check` (svelte-check `--fail-on-warnings`) | PASS: 513 files, 0 errors, 0 warnings |
| `pnpm build` | PASS |
| `develop` CI, last 30 runs | 23 green and 5 red at the start, plus 36c4907 which ended red (6 red); 04f2967 ended green |

The ignored ones are the `#[ignore = "hardware"]` tests and the latency benches.

**CI: FAIL (T-160).** All failures are flaky tests, not regressions; all three pass locally:

| Test | Job | Red runs |
|---|---|---|
| `tandem-input/tests/session.rs::latency_includes_the_wait_with_the_driver_busy` | macos | 72ca3bc, 5703559 |
| `tandemd/tests/pairing.rs::pairing_hot_opens_the_input_session` | macos | 59dd296, f327049 |
| `tandem-cli/tests/e2e.rs::an_mcp_client_opens_a_tunnel_runs_reads_copies_and_closes` | ubuntu, macos, coverage | e551446, 36c4907 |

The run of 04f2967 (the evaluated commit) ended green on Linux, Windows and macOS, plus UI x3, coverage, `.dmg` and installer. But the run of the previous commit (36c4907) was red because of the MCP test, and with three tests that fail at random in CI a commit's green depends on luck, so G11 is not considered met until T-160.

## 2. Non-visual rules G1-G11 and documentation consistency

| Rule | Result | Evidence |
|---|---|---|
| G1 two machines | FAIL | `states.ts` ("First time" state) draws the "Add machine" button and the text "Add another machine"; D1 excludes them. T-161 |
| G2 nothing fixed to Mac/PC | PASS | `Machine::{A,B}`; tests with same-system pairs in `tandem-sim/tests/power.rs`, `tandemd/tests/input.rs`, `tandem-env`; in the UI the icon comes from the system (`machine.test.ts`) |
| G3 sidebar | not evaluated here (visual, part A); `shell.test.ts` tests it |
| G4 buttons, chips | part A |
| G5 no filler or "Keyboard here" / "requires cable" | FAIL only because of the G1 text; `git grep` finds no "Keyboard here" or "requires cable" in `apps/` or `crates/` |
| G6 everything does something real | part B |
| G7, G8 animations | part A; `prefers-reduced-motion` appears in 17 UI files and in `styles/global.css` |
| G9 no console errors | part B (the `shots.ts` harness checks console errors) |
| G10 shared configuration and "Undo" | covered in the agent by `tandemd/tests/network.rs` (`UndoDesk`, `undo_to`); the UI part belongs to part B |
| G11 quality | FAIL because of the flaky CI (T-160) |

Documentation consistency (DECISIONS, DESIGN, ACCEPTANCE and tasks):

- DECISIONS and DESIGN do not contradict each other in what I reviewed (A1, A2, A6, A7, D1-D20 and the agents' notes agree; T-098 "only keyboard and clipboard are turned off" was extended by T-115 with the `tunnels` module, and T-098 documents it as of that moment).
- FAIL (T-163): `docs/STATUS.md` is the v0.2 cut and contradicts A2, D7, D11 and D2; `docs/TESTING.md:50-55` uses `Machine::Mac/Pc` and `set_mode` (against A1 and D6); `docs/dev/hardware.md:7` says "no test changes inputs" (against A8, T-110 and T-123).
- FAIL (T-164): `done` tasks with unchecked criteria or outside the protocol: T-051, T-115, T-123 (two criteria that are hardware), T-128 (heading `## Pending verification` and a non-hardware item), T-012 and T-022.

## 3. Plan v0.3 (T-093 to T-131)

All 39 files exist. 36 are `done`. The others:

| Task | Status | Justification |
|---|---|---|
| T-095 | blocked | Replaced by T-109, T-110 and T-111 (note of 2026-11-16) |
| T-096 | blocked | Replaced by T-112 to T-120 |
| T-124 | todo | It is this acceptance (parts A, B and C); it closes with `FINAL_OK` |

T-088 (outside the range) is also replaced (T-110). No `done` task depends on a `blocked` one. Except for what is in T-164, no `done` task from T-093 to T-131 has unmet criteria outside its hardware list.

"Pending hardware verification" lists: those of T-093, T-094, T-096, T-104, T-105, T-108, T-109, T-110, T-111, T-121, T-122, T-123 and T-131 are concrete (which command, which result, on which machine). T-128's is not (T-164).

## 4. Hardware items

| Item | Status | Where its check is |
|---|---|---|
| H1 pair over Wi-Fi | pending hardware | T-123, first item (`scripts/hw/run.sh`, pairs with test identities); T-093 |
| H2 PC→Mac and Mac→PC tunnels | pending hardware | T-104 (WSL and PowerShell from the Mac), T-108, T-123 |
| H3 34" with automatic rollback | pending hardware | T-109, T-110 (kill the test halfway), T-123 with `--with-monitors` |
| H4 real crossing with synthetic input | pending hardware | T-123 ("synthetic `SendInput`/`CGEventPost` really cross") and T-093 |
| H5 USB-C | pending hardware | T-094 (spike S5 and TB4/USB4 cable, A6) |

They count as pending, not as failures: each one has its concrete check. The T-123 suite has not run on the real machines yet (only `--sim`, 12/12); that is why no `[hw]` item is verified with a date.

## 5. Security

| Check | Result |
|---|---|
| Secrets, tokens and keys | PASS: none versioned; the `ghp_x` and `BEGIN OPENSSH PRIVATE KEY` are fixtures of the credential filter (T-130); there is no `.env`, `.pem` or identities in `git ls-files` |
| User data | FAIL (T-162): the repo is public and carries the Mac's IP (`<mac-ip>`) and its macOS user (`<user>`) in `docs/dev/hardware.md`, `docs/tasks/T-051.md` and `docs/dev/HANDOFF.md`; MAC and `<personal path>` path in fixtures |
| `unsafe` with `// SAFETY:` | FAIL (T-165): of 162 uses, 155 have it; 7 are missing (`extern` of IOKit, ApplicationServices and Carbon, `tap_callback`, `unsafe impl` in `drag_macos.rs`) |
| Copied GPL code | PASS, within the limit of a search-based review: no GPL headers or "copied from"; the only mentions of Deskflow are behavior comments ("like Deskflow") |

## Fix tasks

| Task | Failure |
|---|---|
| [T-160](../tasks/T-160.md) | Flaky tests in CI (macOS and Ubuntu) |
| [T-161](../tasks/T-161.md) | "Add machine" in the "First time" state (D1, G1, G5) |
| [T-162](../tasks/T-162.md) | LAN IP and Mac user in the public repo |
| [T-163](../tasks/T-163.md) | STATUS, TESTING and dev/hardware out of date |
| [T-164](../tasks/T-164.md) | Unchecked criteria and hardware headings |
| [T-165](../tasks/T-165.md) | Missing SAFETY comments |

`ACCEPTANCE: FAIL 6 (tasks: T-160, T-161, T-162, T-163, T-164, T-165)`
