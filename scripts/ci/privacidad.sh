#!/usr/bin/env bash
# Falla si el repo público contiene datos personales: IP de la LAN fuera de las fixtures
# de pruebas, rutas de usuario reales o los identificadores ya retirados (T-162).
set -u
cd "$(git rev-parse --show-toplevel)"
fallo=0
# Identificadores concretos que no deben volver: vienen del secreto PRIVACY_PATTERNS
# (regex extendida), nunca escritos en el repo, que es público.
if [ -n "${PRIVACY_PATTERNS:-}" ]; then
  if git grep -nE "$PRIVACY_PATTERNS" -- . ':!scripts/ci/privacidad.sh'; then
    fallo=1
  fi
fi
# IP privada 192.168.x.x: solo permitida como dato inventado dentro del código de pruebas.
if git grep -nE "192\.168\.[0-9]+\.[0-9]+" \
  -- . ':!crates' ':!docs/reports' ':!scripts/ci/privacidad.sh'; then
  fallo=1
fi
[ "$fallo" -eq 0 ] && echo "privacidad: sin hallazgos"
exit "$fallo"
