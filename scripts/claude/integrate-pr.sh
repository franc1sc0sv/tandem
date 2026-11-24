#!/usr/bin/env bash
# Usage: integrate.sh <pr> <branch>. Merges into develop only if CI passed on a head
# that already contains the current develop. If the branch is behind, it updates it (gh pr
# update-branch, without rewriting history) and waits for its CI. Conflicts: only Cargo.lock.
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
  cd "$wt" || { echo "no worktree $wt"; exit 1; }
  git checkout -q --detach "origin/$br"
  git merge --no-edit origin/develop >/dev/null 2>&1
  others=$(git diff --name-only --diff-filter=U | grep -v '^Cargo.lock$')
  if [[ -n "$others" ]]; then echo "conflicts outside Cargo.lock: $others"; git merge --abort; exit 2; fi
  git checkout --theirs Cargo.lock && cargo check -q --workspace 2>&1 | tail -3
  git add -A && git commit -q --no-edit && git push -q origin "HEAD:$br" || exit 4
  cd "$repo"; wait_ci
elif ! git merge-base --is-ancestor origin/develop "origin/$br"; then
  echo "branch behind: update-branch and CI"
  gh pr update-branch "$pr" >/dev/null 2>&1 || { echo "update-branch failed"; exit 4; }
  wait_ci
fi
if [ "$(gh pr checks "$pr" --json bucket -q 'any(.[]; .bucket=="pending") or length == 0')" = true ]; then wait_ci; fi
gh pr checks "$pr" --json name,bucket -q '[.[] | "\(.name)=\(.bucket)"] | join(" ")'
green || { echo "CI is not green on the current develop"; exit 5; }
gh pr merge "$pr" --squash --delete-branch 2>&1 | grep -iv worktree | grep -v '^$' | tail -1
echo "merge #$pr ok"
