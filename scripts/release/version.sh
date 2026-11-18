#!/usr/bin/env bash
# Prints the Tandem version and checks that it matches in the three places it lives
# (workspace Cargo.toml, tauri.conf.json and the UI's package.json). With a tag, it also
# requires it to be exactly `v<version>`: that way the release never ships installers of a
# different version than the tag. See docs/dev/release.md.
#
# Usage: scripts/release/version.sh [--root <dir>] [v<version>]
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
tag=
while [ $# -gt 0 ]; do
  case "$1" in
    --root) root=$2; shift 2 ;;
    -*) echo "version.sh: unknown option $1" >&2; exit 2 ;;
    *) tag=$1; shift ;;
  esac
done

fail() { echo "version.sh: $*" >&2; exit 1; }

cargo=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\([^"]*\)".*/\1/p;}' "$root/Cargo.toml")
[ -n "$cargo" ] || fail "version not found in [workspace.package] of Cargo.toml"
tauri=$(jq -r '.version // empty' "$root/apps/tandem-ui/src-tauri/tauri.conf.json")
npm=$(jq -r '.version // empty' "$root/apps/tandem-ui/package.json")

[ "$tauri" = "$cargo" ] || fail "tauri.conf.json says '$tauri' and Cargo.toml '$cargo'"
[ "$npm" = "$cargo" ] || fail "package.json says '$npm' and Cargo.toml '$cargo'"
if [ -n "$tag" ] && [ "$tag" != "v$cargo" ]; then
  fail "tag $tag does not match version $cargo (expected v$cargo)"
fi
echo "$cargo"
