#!/usr/bin/env bash
# Construye Tandem.app y Tandem_<versión>_<arch>.dmg con tandemd dentro (sidecar de Tauri) y
# comprueba el resultado. Solo en macOS. Ver docs/dev/install-macos.md.
#
# Firma: ad-hoc por omisión (`signingIdentity: "-"` en tauri.macos.conf.json). Para firmar con
# Developer ID y notarizar, las variables de entorno del bundler de Tauri (APPLE_SIGNING_IDENTITY,
# APPLE_CERTIFICATE, APPLE_ID / APPLE_API_KEY...), descritas en la guía.
#
# Uso: scripts/macos/bundle.sh [argumentos extra para `tauri build`]
#      TANDEM_TARGET=x86_64-apple-darwin scripts/macos/bundle.sh   # otra arquitectura
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
target=${TANDEM_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}
case "$target" in
  *-apple-darwin) ;;
  *) echo "bundle.sh: solo para macOS (target $target)" >&2; exit 2 ;;
esac

ui="$root/apps/tandem-ui"
bin="$ui/src-tauri/binaries"

# 1. tandemd para el mismo target que la app. Tauri busca el sidecar con el sufijo del target
#    y lo copia a Contents/MacOS/tandemd (sin sufijo).
cargo build --release --locked -p tandemd --target "$target" --manifest-path "$root/Cargo.toml"
mkdir -p "$bin"
cp "$root/target/$target/release/tandemd" "$bin/tandemd-$target"

# 2. La app y la imagen de disco. `tauri.sidecar.json` agrega el sidecar solo aquí, para que
#    `cargo build` de la app no lo necesite.
cd "$ui"
pnpm tauri build --target "$target" --bundles app,dmg --features app \
  --config src-tauri/tauri.sidecar.json "$@"

# 3. Lo que tiene que haber quedado.
bundle="$root/target/$target/release/bundle"
app="$bundle/macos/Tandem.app"
shopt -s nullglob
dmgs=("$bundle"/dmg/Tandem_*.dmg)
fail() { echo "bundle.sh: $*" >&2; exit 1; }
[ -d "$app" ] || fail "no se generó $app"
[ ${#dmgs[@]} -eq 1 ] || fail "se esperaba un Tandem_*.dmg en $bundle/dmg"
[ -x "$app/Contents/MacOS/tandemd" ] || fail "falta tandemd dentro de la app"
"$app/Contents/MacOS/tandemd" --version
/usr/libexec/PlistBuddy -c 'Print :NSLocalNetworkUsageDescription' "$app/Contents/Info.plist" >/dev/null \
  || fail "Info.plist sin NSLocalNetworkUsageDescription"
codesign --verify --deep --strict --verbose=2 "$app"
codesign --display --verbose=2 "$app" 2>&1 | grep -E '^(Identifier|Authority|Signature)=' || true
hdiutil verify "${dmgs[0]}" >/dev/null
echo "listo: ${dmgs[0]}"
