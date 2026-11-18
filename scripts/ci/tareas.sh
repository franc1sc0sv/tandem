#!/usr/bin/env bash
# Falla si una tarea `done` deja `- [ ]` fuera de las secciones de hardware o de notas (T-193).
set -u
cd "$(git rev-parse --show-toplevel)"
fallo=0
for f in docs/tasks/T-*.md; do
  grep -q '^status: *done' "$f" || continue
  awk -v f="$f" '
    /^## / { h = ($0 ~ /Verificación en hardware|Notas/) }
    !h && /^[ \t]*- \[ \]/ { printf "%s:%d: tarea done con criterio sin marcar: %s\n", f, NR, $0; bad = 1 }
    END { exit bad }' "$f" || fallo=1
done
[ "$fallo" -eq 0 ] && echo "tareas: sin hallazgos"
exit "$fallo"
