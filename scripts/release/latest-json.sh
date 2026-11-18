#!/usr/bin/env bash
# Builds the `latest.json` read by the Tauri updater (T-122): the version, the notes and, for each
# platform, the package URL in the release and its minisign signature (the `.sig` that
# `tauri build` leaves with `createUpdaterArtifacts`). See docs/dev/release.md.
#
# Usage: scripts/release/latest-json.sh --version x.y.z --repo owner/repo --tag vx.y.z \
#        [--notes notes.md] [--date 2026-11-17T12:00:00Z] platform=package ...
#   platform: darwin-aarch64, darwin-x86_64, windows-x86_64 or linux-x86_64
#   package: the file uploaded to the release; its signature must be in <package>.sig
set -euo pipefail

version= repo= tag= notes= date=
packages=()
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=$2; shift 2 ;;
    --repo) repo=$2; shift 2 ;;
    --tag) tag=$2; shift 2 ;;
    --notes) notes=$2; shift 2 ;;
    --date) date=$2; shift 2 ;;
    -*) echo "latest-json.sh: unknown option $1" >&2; exit 2 ;;
    *) packages+=("$1"); shift ;;
  esac
done

fail() { echo "latest-json.sh: $*" >&2; exit 1; }
[ -n "$version" ] || fail "missing --version"
[ -n "$repo" ] || fail "missing --repo"
[ -n "$tag" ] || fail "missing --tag"
[ ${#packages[@]} -gt 0 ] || fail "missing at least one platform=package"
[ -z "$date" ] && date=$(date -u +%Y-%m-%dT%H:%M:%SZ)
body=
if [ -n "$notes" ]; then
  [ -f "$notes" ] || fail "$notes does not exist"
  body=$(cat "$notes")
fi

platforms='{}'
for pair in "${packages[@]}"; do
  platform=${pair%%=*}
  package=${pair#*=}
  case "$platform" in
    darwin-aarch64 | darwin-x86_64 | windows-x86_64 | linux-x86_64) ;;
    *) fail "unknown platform \"$platform\" in \"$pair\"" ;;
  esac
  [ "$platform" != "$pair" ] || fail "\"$pair\" is not platform=package"
  [ -f "$package" ] || fail "package $package does not exist"
  [ -s "$package.sig" ] || fail "missing signature $package.sig (was it built with TAURI_SIGNING_PRIVATE_KEY?)"
  if jq -e --arg p "$platform" 'has($p)' <<<"$platforms" >/dev/null; then
    fail "repeated platform: $platform"
  fi
  url="https://github.com/$repo/releases/download/$tag/$(basename "$package")"
  platforms=$(jq --arg p "$platform" --arg url "$url" --rawfile sig "$package.sig" \
    '. + {($p): {signature: ($sig | rtrimstr("\n")), url: $url}}' <<<"$platforms")
done

jq -n --arg version "$version" --arg notes "$body" --arg date "$date" \
  --argjson platforms "$platforms" \
  '{version: $version, notes: $notes, pub_date: $date, platforms: $platforms}'
