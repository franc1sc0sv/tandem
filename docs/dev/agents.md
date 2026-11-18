# How the agents work in Tandem

## Pieces

| Piece | File | What it does |
|---|---|---|
| Project rules | `CLAUDE.md`, `.claude/rules/` | Loaded in every session and every agent |
| Backlog | `docs/tasks/T-*.md` | One task per file, with `status`, `depends_on`, `needs` |
| implementer | `.claude/agents/implementer.md` | Implements a task in its own worktree and opens a draft PR |
| reviewer | `.claude/agents/reviewer.md` | Reviews the PR (read-only) and comments |
| hardware-verifier | `.claude/agents/hardware-verifier.md` | Tests with the PC or the Mac; only if the user asks |
| Workflow | `.claude/workflows/next-tasks.js` | Picks the ready tasks → implement → review → fix |
| Quality gate | `scripts/claude/gate-commit.sh` + hook in `.claude/settings.json` | Blocks `git commit` and `gh pr create` if fmt, clippy or the tests fail |
| CI | `.github/workflows/ci.yml` | The same on Linux, Windows and macOS for each PR |

## Cycle

1. Open Claude Code **in this folder** (`apps/tandem`) so the agents, the workflow and the permissions load.
2. Ask: `run /next-tasks` (or `/next-tasks` with `{ "n": 3 }`).
3. Each task ends in a **draft PR** with the review as a comment.
4. You review and merge. **The merge is what unblocks** the tasks that depend on that one.
5. Repeat. To keep it going on its own while you are away: `claude --bg "run /next-tasks"` from a terminal in this folder, or `/loop run /next-tasks` in an open session.

## What they do not do on their own

- Tasks with `needs` other than `none` (hardware or user): done in a session with you.
- Merge to `main`, force-push, disabling commit signing: forbidden by permissions.

## Parallelism

At most 3 tasks at once (2 by default). Each worktree builds its own `target/` and WSL has 24 GB.
