#!/usr/bin/env bash
# Falla si el repo público contiene datos personales: IP de la LAN fuera de las fixtures
# de pruebas, rutas de usuario reales o los identificadores ya retirados (T-162).
set -u
cd "$(git rev-parse --show-toplevel)"
fallo=0
# Identificadores concretos que no deben volver.
if git grep -nE "franciscohernandez|192\.168\.1\.191|9c:6b:00|/Users/fran/|/home/francisco" \
  -- . ':!docs/reports' ':!docs/tasks/T-162.md' ':!scripts/ci/privacidad.sh'; then
  fallo=1
fi
# IP privada 192.168.x.x: solo permitida como dato inventado dentro del código de pruebas.
if git grep -nE "192\.168\.[0-9]+\.[0-9]+" \
  -- . ':!crates' ':!docs/reports' ':!docs/tasks/T-162.md' ':!scripts/ci/privacidad.sh'; then
  fallo=1
fi
[ "$fallo" -eq 0 ] && echo "privacidad: sin hallazgos"
exit "$fallo"
