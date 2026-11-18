#!/usr/bin/env bash
# PreToolUse hook: blocks `git commit` / `gh pr create` if fmt, clippy or tests fail.
set -uo pipefail
input=$(cat)
cwd=$(jq -r '.cwd // empty' <<<"$input")
# The hook's `if` field does not always filter: the script decides on its own by looking at the command.
cmd=$(jq -r '.tool_input.command // empty' <<<"$input")
if ! grep -Eq '(^|[;&|[:space:]])(git([[:space:]]+-C[[:space:]]+[^[:space:]]+)?[[:space:]]+commit|gh[[:space:]]+pr[[:space:]]+create)([[:space:]]|$)' <<<"$cmd"; then
  exit 0
fi
[ -n "$cwd" ] && cd "$cwd"
# Never attribute to Claude/Anthropic (the user's global CLAUDE.md overrides the harness).
if grep -Eqi 'co-authored-by:[^"]*(claude|anthropic)|generated with \[?claude|noreply@anthropic' <<<"$cmd"; then
  echo "Blocked: the message carries Claude/Anthropic attribution. Remove Co-Authored-By and \"Generated with Claude Code\"." >&2
  exit 2
fi
# Outside a Cargo workspace (e.g. docs-only commits before T-001) there is nothing to check.
[ -f Cargo.toml ] || exit 0

# The hook runs in its own process, without the agent's environment: without this it would build in each
# worktree's `target/`, with incremental and full debug info (~7-8 GB per worktree). The WSL disk
# lives on C:, so that fills it up (docs/dev/night-run.md, "Disk and build").
# Same target as the task agent: branch task/T-099 -> ~/.cache/tandem-target-t099.
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)
  case "$branch" in
    task/T-*) slot="t${branch#task/T-}" ;;
    *) slot="gate-$(basename "$(git rev-parse --show-toplevel 2>/dev/null || pwd)")" ;;
  esac
  export CARGO_TARGET_DIR="$HOME/.cache/tandem-target-${slot,,}"
fi
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}"
export CARGO_PROFILE_TEST_DEBUG="${CARGO_PROFILE_TEST_DEBUG:-line-tables-only}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"

if ! out=$( { cargo fmt --all -- --check \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo test --workspace; } 2>&1 ); then
  { echo "Quality gate failed. Fix it before committing or opening the PR:"; echo "$out" | tail -n 60; } >&2
  exit 2
fi
exit 0
