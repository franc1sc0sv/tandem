#!/usr/bin/env bash
# Uso: integrate.sh <pr> <rama>. Integra a develop solo si el CI pasó sobre una cabeza
# que ya contiene el develop actual. Si la rama está atrasada, la actualiza (gh pr
# update-branch, sin reescribir historia) y espera su CI. Conflictos: solo Cargo.lock.
set -uo pipefail
pr=$1; br=$2; repo=~/projects/personal-projects/apps/tandem
wt="$repo/.claude/worktrees/${br//\//-}"
cd "$repo"
state() { gh pr view "$pr" --json mergeable,mergeStateStatus -q '"\(.mergeable) \(.mergeStateStatus)"'; }
green() { [ "$(gh pr checks "$pr" --json bucket -q 'all(.[]; .bucket=="pass" or .bucket=="skipping") and any(.[]; .bucket=="pass")')" = true ]; }
wait_ci() { sleep 20; gh pr checks "$pr" --watch --interval 30 >/dev/null 2>&1; }
st=$(state); for i in $(seq 10); do [[ "$st" == UNKNOWN* ]] || break; sleep 5; st=$(state); done
echo "#$pr $br: $st"
git fetch -q origin
if [[ "$st" == CONFLICTING* ]]; then
  cd "$wt" || { echo "sin worktree $wt"; exit 1; }
  git checkout -q --detach "origin/$br"
  git merge --no-edit origin/develop >/dev/null 2>&1
  others=$(git diff --name-only --diff-filter=U | grep -v '^Cargo.lock$')
  if [[ -n "$others" ]]; then echo "conflictos fuera de Cargo.lock: $others"; git merge --abort; exit 2; fi
  git checkout --theirs Cargo.lock && cargo check -q --workspace 2>&1 | tail -3
  git add -A && git commit -q --no-edit && git push -q origin "HEAD:$br" || exit 4
  cd "$repo"; wait_ci
elif ! git merge-base --is-ancestor origin/develop "origin/$br"; then
  echo "rama atrasada: update-branch y CI"
  gh pr update-branch "$pr" >/dev/null 2>&1 || { echo "update-branch falló"; exit 4; }
  wait_ci
fi
if [ "$(gh pr checks "$pr" --json bucket -q 'any(.[]; .bucket=="pending") or length == 0')" = true ]; then wait_ci; fi
gh pr checks "$pr" --json name,bucket -q '[.[] | "\(.name)=\(.bucket)"] | join(" ")'
green || { echo "CI no está en verde sobre develop actual"; exit 5; }
gh pr merge "$pr" --squash --delete-branch 2>&1 | grep -iv worktree | grep -v '^$' | tail -1
echo "merge #$pr ok"
