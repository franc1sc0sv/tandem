#!/usr/bin/env bash
# Falla si un `unsafe` no lleva `SAFETY` en las 6 líneas previas o en la misma (T-165).
# Excluye `#[unsafe(...)]` (atributos) y `unsafe` dentro de comentarios y cadenas.
set -u
cd "$(git rev-parse --show-toplevel)"
fallo=0
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
        if (!ok) { printf "%s:%d: unsafe sin SAFETY: %s\n", f, i, s; bad = 1 }
      }
      exit bad
    }' "$f" || fallo=1
done < <(git ls-files '*.rs')
[ "$fallo" -eq 0 ] && echo "safety: sin hallazgos"
exit "$fallo"
