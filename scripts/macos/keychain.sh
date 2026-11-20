#!/usr/bin/env bash
# Imports the macOS signing identity (T-348) into a temporary keychain for one build, and removes
# it afterwards. macOS only. See docs/dev/release.md#macos-signing-identity.
#
#   scripts/macos/keychain.sh import <identity.p12> <password-file> <keychain-path>
#       Creates the keychain, imports the .p12 so that codesign can use it without a prompt,
#       puts it first in the user's search list and prints the identity's SHA-1 on stdout:
#       that is what goes in APPLE_SIGNING_IDENTITY (Tauri passes it as is to `codesign -s`,
#       and a hash cannot match another identity by name).
#   scripts/macos/keychain.sh disposable <keychain-path>
#       The same with a throwaway identity made on the spot (make-macos-identity.sh): PR CI and
#       the release's test mode sign with it, so the whole path runs without the real secret.
#   scripts/macos/keychain.sh remove <keychain-path>
#       Takes it out of the search list and deletes it. Missing is not an error.
set -euo pipefail

die() { echo "keychain.sh: $*" >&2; exit 1; }
[ "$(uname -s)" = Darwin ] || die "macOS only"

here=$(cd "$(dirname "$0")" && pwd)
cmd=${1:-}
case "$cmd" in
  disposable)
    [ $# -eq 2 ] || die "usage: keychain.sh disposable <keychain-path>"
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    "$here/../release/make-macos-identity.sh" --out "$work" --name "Tandem Disposable Code Signing" --days 2 --quiet
    "$0" import "$work/identity.p12" "$work/password" "$2"
    ;;
  import)
    [ $# -eq 4 ] || die "usage: keychain.sh import <identity.p12> <password-file> <keychain-path>"
    p12=$2 passfile=$3 keychain=$4
    [ -s "$p12" ] || die "no .p12 at $p12"
    [ -f "$passfile" ] || die "no password file at $passfile"
    pass=$(cat "$passfile")
    # The keychain's own password only lives for this build.
    kpass=$(openssl rand -hex 24)
    security create-keychain -p "$kpass" "$keychain"
    # No auto-lock during a long build.
    security set-keychain-settings "$keychain"
    security unlock-keychain -p "$kpass" "$keychain"
    security import "$p12" -k "$keychain" -f pkcs12 -P "$pass" -T /usr/bin/codesign >/dev/null
    # Lets codesign use the key without the "allow access" dialog (none can be answered in CI).
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$kpass" "$keychain" >/dev/null
    # First in the search list, keeping the ones already there: codesign looks the identity up there.
    existing=()
    while IFS= read -r k; do
      k=${k#"${k%%[![:space:]]*}"}; k=${k#\"}; k=${k%\"}
      [ -n "$k" ] && existing+=("$k")
    done < <(security list-keychains -d user)
    security list-keychains -d user -s "$keychain" "${existing[@]}"
    # The identity is the certificate that came with a private key. `find-identity` without `-v`:
    # a self-signed certificate is not trusted, and codesign does not need it to be.
    sha1=$(security find-identity -p codesigning "$keychain" | awk '/^ *1\) /{print $2; exit}')
    [ -n "$sha1" ] || die "the .p12 has no code-signing identity (certificate + private key)"
    echo "$sha1"
    ;;
  remove)
    [ $# -eq 2 ] || die "usage: keychain.sh remove <keychain-path>"
    keychain=$2
    [ -e "$keychain" ] || [ -e "$keychain-db" ] || exit 0
    security delete-keychain "$keychain" || true
    ;;
  *)
    die "usage: keychain.sh import|remove ..."
    ;;
esac
