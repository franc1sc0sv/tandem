#!/usr/bin/env bash
# Fails if the public repo contains personal data: LAN IPs outside test fixtures,
# real user paths or identifiers that were already retired (T-162).
set -u
cd "$(git rev-parse --show-toplevel)"
failed=0
# Specific identifiers that must not come back: they come from the PRIVACY_PATTERNS secret
# (extended regex), never written in the repo, which is public.
if [ -n "${PRIVACY_PATTERNS:-}" ]; then
  if git grep -nE "$PRIVACY_PATTERNS" -- . ':!scripts/ci/privacy.sh'; then
    failed=1
  fi
fi
# Private IP 192.168.x.x: only allowed as made-up data inside test code.
if git grep -nE "192\.168\.[0-9]+\.[0-9]+" \
  -- . ':!crates' ':!docs/reports' ':!scripts/ci/privacy.sh'; then
  failed=1
fi
[ "$failed" -eq 0 ] && echo "privacy: no findings"
exit "$failed"
