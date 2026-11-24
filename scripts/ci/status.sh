#!/usr/bin/env bash
# Fails if the header of docs/STATUS.md does not match docs/tasks/ (T-192):
# done count and total, last task, and every open task (todo/blocked) must appear in the table.
set -u
cd "$(dirname "$0")/../.."
total=$(ls docs/tasks/T-*.md | wc -l)
done_count=$(grep -l '^status: *done' docs/tasks/T-*.md | wc -l)
last=$(ls docs/tasks/T-*.md | sed 's#.*/##; s#\.md##' | sort | tail -1)
expected="**$done_count done of $total** (\`T-001\`…\`$last\`"
failed=0
if ! grep -qF -- "$expected" docs/STATUS.md; then
  echo "STATUS.md: the header must say: $expected"
  failed=1
fi
for f in $(grep -lE '^status: *(todo|blocked|in-progress)' docs/tasks/T-*.md); do
  id=$(basename "$f" .md)
  awk '/^## Open tasks/{a=1;next} /^## /{a=0} a' docs/STATUS.md | grep -q "$id" \
    || { echo "STATUS.md: $id is open and missing from \"Open tasks\""; failed=1; }
done
[ "$failed" -eq 0 ] && echo "status: no findings"
exit "$failed"
