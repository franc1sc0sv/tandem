#!/usr/bin/env bash
# Builds Tandem.app and Tandem_<version>_<arch>.dmg with tandemd inside (Tauri sidecar) and
# checks the result. macOS only. See docs/dev/install-macos.md.
#
# Signing: ad hoc by default (`signingIdentity: "-"` in tauri.macos.conf.json), for local builds.
# CI and the release set APPLE_SIGNING_IDENTITY to a self-signed identity imported with
# scripts/macos/keychain.sh (the stable one in a release, T-348); scripts/macos/dr-check.sh then
# checks that the designated requirement survives a new version. Developer ID and notarization
# use the same Tauri variables (APPLE_SIGNING_IDENTITY, APPLE_ID / APPLE_API_KEY...), see the guide.
#
# Usage: scripts/macos/bundle.sh [extra arguments for `tauri build`]
#      TANDEM_TARGET=x86_64-apple-darwin scripts/macos/bundle.sh   # another architecture
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
target=${TANDEM_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}
case "$target" in
  *-apple-darwin) ;;
  *) echo "bundle.sh: macOS only (target $target)" >&2; exit 2 ;;
esac

ui="$root/apps/tandem-ui"
bin="$ui/src-tauri/binaries"

# 1. tandemd for the same target as the app. Tauri looks for the sidecar with the target suffix
#    and copies it to Contents/MacOS/tandemd (no suffix).
# The `tandem` CLI and MCP server travels in the app too (T-357), so it is always the agent's version.
cargo build --release --locked -p tandemd -p tandem-cli --target "$target" --manifest-path "$root/Cargo.toml"
mkdir -p "$bin"
cp "$root/target/$target/release/tandemd" "$bin/tandemd-$target"
cp "$root/target/$target/release/tandem" "$bin/tandem-$target"

# 2. The app and the disk image. `tauri.sidecar.json` adds the sidecar only here, so that
#    the app's `cargo build` does not need it.
cd "$ui"
pnpm tauri build --target "$target" --bundles app,dmg --features app \
  --config src-tauri/tauri.sidecar.json "$@"

# 3. What must be left behind.
bundle="$root/target/$target/release/bundle"
app="$bundle/macos/Tandem.app"
shopt -s nullglob
dmgs=("$bundle"/dmg/Tandem_*.dmg)
fail() { echo "bundle.sh: $*" >&2; exit 1; }
[ -d "$app" ] || fail "not generated: $app"
[ ${#dmgs[@]} -eq 1 ] || fail "expected one Tandem_*.dmg in $bundle/dmg"
[ -x "$app/Contents/MacOS/tandemd" ] || fail "tandemd missing inside the app"
"$app/Contents/MacOS/tandemd" --version
[ -x "$app/Contents/MacOS/tandem" ] || fail "tandem (CLI) missing inside the app"
"$app/Contents/MacOS/tandem" --version
/usr/libexec/PlistBuddy -c 'Print :NSLocalNetworkUsageDescription' "$app/Contents/Info.plist" >/dev/null \
  || fail "Info.plist without NSLocalNetworkUsageDescription"
codesign --verify --deep --strict --verbose=2 "$app"
codesign --display --verbose=2 "$app" 2>&1 | grep -E '^(Identifier|Authority|Signature)=' || true
# What TCC stores for the permissions (T-348): ad hoc gives `cdhash H"..."`, a new one per build.
codesign -d -r- "$app" 2>&1 | grep '^designated' || true
codesign -d -r- "$app/Contents/MacOS/tandemd" 2>&1 | grep '^designated' || true
hdiutil verify "${dmgs[0]}" >/dev/null
echo "done: ${dmgs[0]}"
