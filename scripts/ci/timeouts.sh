#!/usr/bin/env bash
# Fails if a GitHub Actions job has no `timeout-minutes` (T-191, T-234): a hung step must not
# block a runner for hours.
set -u
cd "$(git rev-parse --show-toplevel)"
fail=0
for f in .github/workflows/*.yml; do
  awk -v f="$f" '
    /^jobs:/ { injobs = 1; next }
    injobs && /^[^ #]/ { injobs = 0 }
    function check() { if (job != "" && !has) { printf "%s: job %s has no timeout-minutes\n", f, job; bad = 1 } }
    injobs && /^  [A-Za-z0-9_-]+:[ \t]*$/ { check(); job = $1; sub(":", "", job); has = 0; next }
    injobs && /^    timeout-minutes:/ { has = 1 }
    END { check(); exit bad }' "$f" || fail=1
done
[ "$fail" -eq 0 ] && echo "timeouts: no findings"
exit "$fail"
