#!/usr/bin/env bash
# Fails if the protocol numbers documented in docs/DESIGN.md (section 4.5, "Protocol versioning")
# differ from the constants in crates/tandem-proto/src/lib.rs (T-260). A PR that bumps
# PROTOCOL_VERSION must update the docs in the same change.
set -u
cd "$(dirname "$0")/../.."
src=crates/tandem-proto/src/lib.rs
doc=docs/DESIGN.md
failed=0
for name in PROTOCOL_VERSION CAPABILITIES_SINCE SIDES_SINCE; do
  code=$(sed -nE "s/^pub const $name: u16 = ([0-9]+);.*/\1/p" "$src")
  if [ -z "$code" ]; then
    echo "protocol: could not read $name from $src"
    failed=1
    continue
  fi
  if ! grep -qF -- "\`$name = $code\`" "$doc"; then
    echo "protocol: $doc must say \`$name = $code\` (the value in $src)"
    failed=1
  fi
  # No other value of the same constant may remain in the docs.
  if grep -hoE "\`$name = [0-9]+\`" "$doc" | grep -vFx -- "\`$name = $code\`" | grep -q .; then
    echo "protocol: $doc still mentions an old value of $name"
    failed=1
  fi
done
[ "$failed" -eq 0 ] && echo "protocol: no findings"
exit "$failed"
