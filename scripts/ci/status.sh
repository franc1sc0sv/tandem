#!/usr/bin/env bash
# Falla si el encabezado de docs/STATUS.md no coincide con docs/tasks/ (T-192):
# conteo de hechas y total, última tarea, y que toda tarea abierta (todo/blocked) aparezca en la tabla.
set -u
cd "$(dirname "$0")/../.."
total=$(ls docs/tasks/T-*.md | wc -l)
hechas=$(grep -l '^status: *done' docs/tasks/T-*.md | wc -l)
ultima=$(ls docs/tasks/T-*.md | sed 's#.*/##; s#\.md##' | sort | tail -1)
esperado="**$hechas hechas de $total** (\`T-001\`…\`$ultima\`"
fallo=0
if ! grep -qF -- "$esperado" docs/STATUS.md; then
  echo "STATUS.md: el encabezado debe decir: $esperado"
  fallo=1
fi
for f in $(grep -lE '^status: *(todo|blocked|in-progress)' docs/tasks/T-*.md); do
  id=$(basename "$f" .md)
  awk '/^## Tareas abiertas/{a=1;next} /^## /{a=0} a' docs/STATUS.md | grep -q "$id" \
    || { echo "STATUS.md: $id está abierta y falta en «Tareas abiertas»"; fallo=1; }
done
[ "$fallo" -eq 0 ] && echo "status: sin hallazgos"
exit "$fallo"
