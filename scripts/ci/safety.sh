#!/usr/bin/env bash
# Fails if an `unsafe` has no `SAFETY` in the 6 preceding lines or on the same line (T-165).
# Excludes `#[unsafe(...)]` (attributes) and `unsafe` inside comments and strings.
set -u
cd "$(git rev-parse --show-toplevel)"
failed=0
while IFS= read -r f; do
  awk -v f="$f" '
    { l[NR] = $0 }
    END {
      for (i = 1; i <= NR; i++) {
        s = l[i]
        if (s ~ /^[ \t]*(\/\/|\*|#)/) continue
        if (s !~ /(^|[^A-Za-z_#\[])unsafe[ \t]+(extern|impl|fn|\{|trait)|[=(,][ \t]*unsafe[ \t]*\{|^[ \t]*unsafe[ \t]*\{/) continue
        ok = (s ~ /SAFETY/)
        for (j = i - 1; j >= 1 && j >= i - 6 && !ok; j--) if (l[j] ~ /SAFETY|# Safety/) ok = 1
        if (!ok) { printf "%s:%d: unsafe without SAFETY: %s\n", f, i, s; bad = 1 }
      }
      exit bad
    }' "$f" || failed=1
done < <(git ls-files '*.rs')
[ "$failed" -eq 0 ] && echo "safety: no findings"
exit "$failed"
