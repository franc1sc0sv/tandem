#!/usr/bin/env bash
# Hook PreToolUse: bloquea `git commit` / `gh pr create` si fmt, clippy o las pruebas fallan.
set -uo pipefail
input=$(cat)
cwd=$(jq -r '.cwd // empty' <<<"$input")
# El campo `if` del hook no siempre filtra: el script decide solo, mirando el comando.
cmd=$(jq -r '.tool_input.command // empty' <<<"$input")
if ! grep -Eq '(^|[;&|[:space:]])(git([[:space:]]+-C[[:space:]]+[^[:space:]]+)?[[:space:]]+commit|gh[[:space:]]+pr[[:space:]]+create)([[:space:]]|$)' <<<"$cmd"; then
  exit 0
fi
[ -n "$cwd" ] && cd "$cwd"
# Fuera de un workspace de Cargo (p. ej. commits solo de docs antes de T-001) no hay nada que revisar.
[ -f Cargo.toml ] || exit 0

# El hook corre en su propio proceso, sin el entorno del agente: sin esto compilaría en `target/`
# de cada worktree, con incremental y depuración completa (~7-8 GB por worktree). El disco de WSL
# vive en C:, así que eso lo llena (docs/dev/night-run.md, «Disco y compilación»).
# Mismo target que el agente de la tarea: rama task/T-099 → ~/.cache/tandem-target-t099.
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
  { echo "Puerta de calidad fallida. Corrige antes de hacer commit o abrir el PR:"; echo "$out" | tail -n 60; } >&2
  exit 2
fi
exit 0
