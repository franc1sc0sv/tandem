#!/usr/bin/env bash
# Fails if a `done` task leaves `- [ ]` outside the hardware or notes sections (T-193).
set -u
cd "$(git rev-parse --show-toplevel)"
failed=0
for f in docs/tasks/T-*.md; do
  grep -q '^status: *done' "$f" || continue
  awk -v f="$f" '
    /^## / { h = ($0 ~ /Pending hardware verification|Notes/) }
    !h && /^[ \t]*- \[ \]/ { printf "%s:%d: done task with unchecked criterion: %s\n", f, NR, $0; bad = 1 }
    END { exit bad }' "$f" || failed=1
done
[ "$failed" -eq 0 ] && echo "tasks: no findings"
exit "$failed"
