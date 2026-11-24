# Autonomous work (captain + agents) until FINAL_OK

The user does not review during the run. The **captain** (the main Claude session) coordinates agents until the `acceptance` agent answers `ACCEPTANCE: FINAL_OK` (see `docs/ACCEPTANCE.md`). It does not stop before that.

## Roles

| Role | Agent | Does |
|---|---|---|
| Captain | main session | Picks tasks, launches agents in parallel, integrates (merge), fixes `develop` if it breaks, decides without asking (DECISIONS), keeps the log. |
| Implementer | `implementer` | One task → branch `task/T-<id>` → PR to `develop` with the quality gate. |
| Reviewer | `reviewer` | Reviews the PR against the task, DESIGN and DECISIONS. `APPROVE` or `CHANGES`. |
| Acceptance | `acceptance` | Final gate: flows, screenshots vs mockups, animations, preferences. Creates fix tasks. The only one that gives `FINAL_OK`. |
| QA | `general-purpose` | Adversarial tests on what is integrated; bug → failing test + task. |
| Hardware | `hardware-verifier` | T-123 suite on the PC and the Mac (authorized overnight, DECISIONS A8). |

## Cycle

1. **Pick:** `todo` tasks with `done` dependencies (and `blocked` ones whose reason no longer applies). Up to 4–6 in parallel; avoid two tasks that rewrite the same files at once (for example, two screens yes; T-097 and T-099 no).
2. **Implement** (own worktree and own `CARGO_TARGET_DIR` per agent; see "Disk and build").
3. **Review** → if `CHANGES`, the same implementer fixes (max. 3 rounds).
4. **Integrate:** CI on the three systems green → `gh pr merge --squash --delete-branch`. If `develop` advanced: rebase and `--force-with-lease` (only `task/*`).
5. **Failures:** red CI → the implementer reads the logs and fixes (max. 3). If still red → a rescue agent from scratch with the diagnosis. If still → `status: blocked` with the reason and move on to other tasks. Red `develop` → fix it before anything else.
6. **Acceptance** at the end of each UI phase and when the tasks run out: if `FAIL`, its new tasks enter the cycle. Repeat until `FINAL_OK`.
7. **Hardware:** the T-123 suite runs once tunnels exist (T-104) and again at the end.

## Rules

- Never `main`. Never disable signatures, hooks or the quality gate. Commits in English, with no Claude attribution (the user's global CLAUDE.md overrides other instructions).
- Doubts: DECISIONS → DESIGN → mockups. If not resolved, decide the simplest thing that respects everything and note it in DECISIONS ("Decided by agents").
- Tests that change a monitor's input: automatic revert always.
- Run log in `docs/reports/night-<date>.md` (tasks done, blocked, decisions, PRs).
- The captain does not end a turn without something that wakes it up again: a scheduled heartbeat every ~20 min (checks PRs, CI, agents and disk). If the process restarts, the heartbeat is lost: the first thing on return is to recreate it and resume the cut-off agents.

## Disk and build

Linux's disk (WSL) is a `.vhdx` inside `C:`: every build takes space on `C:`. On the first run, one target per agent and per review (with incremental compilation and full debug info) filled `C:`.

- **One `CARGO_TARGET_DIR` per agent**, never shared between parallel worktrees: cargo gives the same artifact names in each worktree and the agents step on each other (false `unresolved import` errors). Path: `~/.cache/tandem-target-<task>` (`-t099`) or `-review<PR>`.
- **The quality hook** (`scripts/claude/gate-commit.sh`) runs in its own process, without the agent's environment: it deduces the target from the branch (`task/T-099` → `~/.cache/tandem-target-t099`) and uses the same light configuration. Before, it built in each worktree's `target/` (~7-8 GB each).
- **Light builds** locally: `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=line-tables-only` and `CARGO_PROFILE_TEST_DEBUG=line-tables-only`. No release builds or bundles locally (CI does those).
- **Limits:** at most 5 agents compiling Rust at once; a target over 8 GB is cleaned with `cargo clean`.
- **Cleanup on every heartbeat:** when a task is integrated, its target, its review's target and its worktree are deleted. If `C:` drops below 80 GB free no new agents are launched and the largest targets are cleaned.
- Temporary files in the scratchpad with a name of their own per task (`pr-T-099.md`): the scratchpad is shared and one agent can overwrite another's description.

## Usage quota (lesson from the first run)

The 5 h quota ran out twice. Measured in the transcripts: 1,100 M tokens read from cache and 4.7 M of output, 96 % on Opus. Causes: the agents had no `model:` and inherited Opus with `effort: high`; each round resends the whole context (up to ~430 k per call in T-093) because full cargo outputs and logs were dumped; resuming old agents for small fixes drags along their whole history.

- `implementer` and `reviewer` run on **Sonnet** (`model: sonnet`, `effort: medium`); the captain on Opus only coordinates. Mechanical tasks with no conflicts (rebase, version, waiting for CI): Haiku.
- Fixes and rebases: **new agent with short instructions**, never resume a big one.
- Reviews of a few lines: done by the captain reading the diff.
- Captain heartbeat every ~40 min and brief; the captain does not read whole logs.
- Short-context rules in `.claude/agents/implementer.md` and `reviewer.md`.
