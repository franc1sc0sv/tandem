#!/usr/bin/env bash
# Imprime la versión de Tandem y comprueba que coincide en los tres lugares donde vive
# (Cargo.toml del workspace, tauri.conf.json y package.json de la UI). Con un tag, además
# exige que sea exactamente `v<versión>`: así el release nunca sale con instaladores de otra
# versión que la del tag. Ver docs/dev/release.md.
#
# Uso: scripts/release/version.sh [--raiz <dir>] [v<versión>]
set -euo pipefail

raiz=$(cd "$(dirname "$0")/../.." && pwd)
tag=
while [ $# -gt 0 ]; do
  case "$1" in
    --raiz) raiz=$2; shift 2 ;;
    -*) echo "version.sh: opción desconocida $1" >&2; exit 2 ;;
    *) tag=$1; shift ;;
  esac
done

fallo() { echo "version.sh: $*" >&2; exit 1; }

cargo=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p;}' "$raiz/Cargo.toml")
[ -n "$cargo" ] || fallo "no encontré version en [workspace.package] de Cargo.toml"
tauri=$(jq -r '.version // empty' "$raiz/apps/tandem-ui/src-tauri/tauri.conf.json")
npm=$(jq -r '.version // empty' "$raiz/apps/tandem-ui/package.json")

[ "$tauri" = "$cargo" ] || fallo "tauri.conf.json dice '$tauri' y Cargo.toml '$cargo'"
[ "$npm" = "$cargo" ] || fallo "package.json dice '$npm' y Cargo.toml '$cargo'"
if [ -n "$tag" ] && [ "$tag" != "v$cargo" ]; then
  fallo "el tag $tag no coincide con la versión $cargo (se esperaba v$cargo)"
fi
echo "$cargo"
