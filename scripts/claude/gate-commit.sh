#!/usr/bin/env bash
# Hook PreToolUse: bloquea `git commit` / `gh pr create` si fmt, clippy o las pruebas fallan.
set -uo pipefail
input=$(cat)
cwd=$(jq -r '.cwd // empty' <<<"$input")
[ -n "$cwd" ] && cd "$cwd"
# Fuera de un workspace de Cargo (p. ej. commits solo de docs antes de T-001) no hay nada que revisar.
[ -f Cargo.toml ] || exit 0
if ! out=$( { cargo fmt --all -- --check \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo test --workspace; } 2>&1 ); then
  { echo "Puerta de calidad fallida. Corrige antes de hacer commit o abrir el PR:"; echo "$out" | tail -n 60; } >&2
  exit 2
fi
exit 0
