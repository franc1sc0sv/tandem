#!/usr/bin/env bash
# Arma el `latest.json` que lee el updater de Tauri (T-122): la versión, las notas y, por cada
# plataforma, la URL del paquete en el release y su firma minisign (el `.sig` que deja
# `tauri build` con `createUpdaterArtifacts`). Ver docs/dev/release.md.
#
# Uso: scripts/release/latest-json.sh --version x.y.z --repo dueño/repo --tag vx.y.z \
#        [--notas notas.md] [--fecha 2026-11-17T12:00:00Z] plataforma=paquete ...
#   plataforma: darwin-aarch64, darwin-x86_64, windows-x86_64 o linux-x86_64
#   paquete: el archivo que se sube al release; su firma tiene que estar en <paquete>.sig
set -euo pipefail

version= repo= tag= notas= fecha=
paquetes=()
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=$2; shift 2 ;;
    --repo) repo=$2; shift 2 ;;
    --tag) tag=$2; shift 2 ;;
    --notas) notas=$2; shift 2 ;;
    --fecha) fecha=$2; shift 2 ;;
    -*) echo "latest-json.sh: opción desconocida $1" >&2; exit 2 ;;
    *) paquetes+=("$1"); shift ;;
  esac
done

fallo() { echo "latest-json.sh: $*" >&2; exit 1; }
[ -n "$version" ] || fallo "falta --version"
[ -n "$repo" ] || fallo "falta --repo"
[ -n "$tag" ] || fallo "falta --tag"
[ ${#paquetes[@]} -gt 0 ] || fallo "falta al menos un plataforma=paquete"
[ -z "$fecha" ] && fecha=$(date -u +%Y-%m-%dT%H:%M:%SZ)
cuerpo=
if [ -n "$notas" ]; then
  [ -f "$notas" ] || fallo "no existe $notas"
  cuerpo=$(cat "$notas")
fi

plataformas='{}'
for par in "${paquetes[@]}"; do
  plataforma=${par%%=*}
  paquete=${par#*=}
  case "$plataforma" in
    darwin-aarch64 | darwin-x86_64 | windows-x86_64 | linux-x86_64) ;;
    *) fallo "plataforma desconocida «$plataforma» en «$par»" ;;
  esac
  [ "$plataforma" != "$par" ] || fallo "«$par» no es plataforma=paquete"
  [ -f "$paquete" ] || fallo "no existe el paquete $paquete"
  [ -s "$paquete.sig" ] || fallo "falta la firma $paquete.sig (¿se construyó con TAURI_SIGNING_PRIVATE_KEY?)"
  if jq -e --arg p "$plataforma" 'has($p)' <<<"$plataformas" >/dev/null; then
    fallo "plataforma repetida: $plataforma"
  fi
  url="https://github.com/$repo/releases/download/$tag/$(basename "$paquete")"
  plataformas=$(jq --arg p "$plataforma" --arg url "$url" --rawfile firma "$paquete.sig" \
    '. + {($p): {signature: ($firma | rtrimstr("\n")), url: $url}}' <<<"$plataformas")
done

jq -n --arg version "$version" --arg notes "$cuerpo" --arg fecha "$fecha" \
  --argjson platforms "$plataformas" \
  '{version: $version, notes: $notes, pub_date: $fecha, platforms: $platforms}'
