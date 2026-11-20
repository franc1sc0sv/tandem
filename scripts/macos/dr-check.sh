#!/usr/bin/env bash
# Checks that the macOS signature keeps the same designated requirement (DR) across versions
# (T-348). TCC stores the DR of the code a person granted Accessibility, Input Monitoring or
# Local Network to, and on each access checks that the running code satisfies it. With ad-hoc
# signing the DR is `cdhash H"..."`, so every build is new to TCC; with the stable identity it is
# `identifier "..." and certificate leaf = H"..."` and any version signed with it satisfies it.
# See docs/dev/release.md#macos-signing-identity.
#
#   scripts/macos/dr-check.sh compare <a> <b>
#       <a> and <b> are two builds of the same code (Tandem.app, or the bare tandemd). Fails if
#       either DR is missing or ad hoc (cdhash), if they differ, if <b> does not satisfy the DR of
#       <a> (what TCC evaluates), or if both have the same cdhash (then it proves nothing).
#   scripts/macos/dr-check.sh derive <identity> <Tandem.app> <out-dir>
#       Makes <out-dir>/Tandem.app: a copy that is a different version (another
#       CFBundleShortVersionString and CFBundleVersion, and a tandemd with another SDK version in
#       its LC_BUILD_VERSION, so both cdhashes change), signed again with <identity> the way
#       Tauri signs (hardened runtime). Prints its path.
#   scripts/macos/dr-check.sh package <Tandem.app> <Tandem.app.tar.gz>
#       The updater package: unpacks it, checks that nothing inside carries the quarantine
#       attribute and that its app and tandemd keep the DR of <Tandem.app>.
#   scripts/macos/dr-check.sh all <identity> <Tandem.app> [<Tandem.app.tar.gz>]
#       derive + compare (app and tandemd) + package. What CI and the release run.
set -euo pipefail

die() { echo "dr-check.sh: $*" >&2; exit 1; }

# The `designated => ...` line of `codesign -d -r-`. Empty if there is none.
dr_of() {
  codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => //p' | head -n 1
}

cdhash_of() {
  codesign -d -vvv "$1" 2>&1 | sed -n 's/^CDHash=//p' | head -n 1
}

compare() {
  local a=$1 b=$2 dra drb
  [ -e "$a" ] || die "missing: $a"
  [ -e "$b" ] || die "missing: $b"
  dra=$(dr_of "$a")
  drb=$(dr_of "$b")
  echo "DR $a"
  echo "   => ${dra:-<none>}"
  echo "DR $b"
  echo "   => ${drb:-<none>}"
  [ -n "$dra" ] || die "no designated requirement in $a (unsigned?)"
  [ -n "$drb" ] || die "no designated requirement in $b (unsigned?)"
  case "$dra$drb" in
    *cdhash*) die "the designated requirement is a cdhash: the build is signed ad hoc, and macOS asks for the permissions again on every version" ;;
  esac
  [ "$dra" = "$drb" ] || die "the designated requirements differ: TCC would treat $b as another app"
  # What TCC does with the stored requirement: the new build must satisfy the old one's DR.
  # `-R` and the text as two arguments: the leading `=` marks it as requirement text, not a file.
  codesign --verify --strict -R "=$dra" "$b" || die "$b does not satisfy the designated requirement of $a"
  local ca cb
  ca=$(cdhash_of "$a")
  cb=$(cdhash_of "$b")
  [ -n "$ca" ] && [ -n "$cb" ] || die "could not read the cdhashes"
  [ "$ca" != "$cb" ] || die "$a and $b have the same cdhash: they are the same build, the check proves nothing"
  echo "ok: same designated requirement, different builds ($ca / $cb)"
}

derive() {
  local identity=$1 app=$2 out=$3
  [ "$(uname -s)" = Darwin ] || die "derive: macOS only"
  [ -d "$app" ] || die "no app at $app"
  mkdir -p "$out"
  local copy
  copy="$out/$(basename "$app")"
  rm -rf "$copy"
  # ditto keeps the bundle exactly (links, permissions, extended attributes).
  ditto "$app" "$copy"
  local plist="$copy/Contents/Info.plist" daemon="$copy/Contents/MacOS/tandemd"
  local version key
  version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")
  for key in CFBundleShortVersionString CFBundleVersion; do
    /usr/libexec/PlistBuddy -c "Set :$key 99.$version" "$plist" 2>/dev/null \
      || /usr/libexec/PlistBuddy -c "Add :$key string 99.$version" "$plist"
  done
  # A different tandemd: same code, another SDK version in its load commands.
  codesign --remove-signature "$daemon"
  local minos
  minos=$(xcrun vtool -show-build "$daemon" | awk '$1 == "minos" {print $2; exit}')
  [ -n "$minos" ] || die "derive: no LC_BUILD_VERSION in tandemd"
  xcrun vtool -set-build-version macos "$minos" 99.0 -replace -output "$daemon.new" "$daemon"
  mv -f "$daemon.new" "$daemon"
  chmod +x "$daemon"
  # Inside first, then the bundle (which seals tandemd), as Tauri does.
  codesign --force --options runtime -s "$identity" "$daemon"
  codesign --force --options runtime -s "$identity" "$copy"
  codesign --verify --deep --strict "$copy"
  echo "$copy"
}

package() {
  local app=$1 tarball=$2
  [ -f "$tarball" ] || die "no package at $tarball"
  local tmp
  tmp=$(mktemp -d)
  # shellcheck disable=SC2064  # the path is fixed now on purpose
  trap "rm -rf '$tmp'" RETURN
  tar -xzf "$tarball" -C "$tmp"
  local unpacked
  unpacked="$tmp/$(basename "$app")"
  [ -d "$unpacked" ] || die "the package does not carry $(basename "$app")"
  # The updater writes these files itself; macOS only quarantines what a quarantine-aware app
  # downloads, so the package must not bring the attribute with it either.
  if xattr -r -l "$unpacked" 2>/dev/null | grep -q 'com.apple.quarantine'; then
    die "the updater package carries com.apple.quarantine: Gatekeeper would stop the updated app"
  fi
  codesign --verify --deep --strict "$unpacked" || die "the app inside the updater package does not verify"
  local dra drb inner
  for inner in "" /Contents/MacOS/tandemd; do
    dra=$(dr_of "$app$inner")
    drb=$(dr_of "$unpacked$inner")
    [ -n "$dra" ] && [ "$dra" = "$drb" ] \
      || die "the updater package's $(basename "$app")$inner has another designated requirement: '$drb' vs '$dra'"
  done
  echo "ok: the updater package keeps the designated requirement and carries no quarantine"
}

case "${1:-}" in
  compare) [ $# -eq 3 ] || die "usage: dr-check.sh compare <a> <b>"; compare "$2" "$3" ;;
  derive) [ $# -eq 4 ] || die "usage: dr-check.sh derive <identity> <Tandem.app> <out-dir>"; derive "$2" "$3" "$4" ;;
  package) [ $# -eq 3 ] || die "usage: dr-check.sh package <Tandem.app> <Tandem.app.tar.gz>"; package "$2" "$3" ;;
  all)
    [ $# -eq 3 ] || [ $# -eq 4 ] || die "usage: dr-check.sh all <identity> <Tandem.app> [<Tandem.app.tar.gz>]"
    identity=$2 app=$3
    out=$(mktemp -d)
    second=$(derive "$identity" "$app" "$out" | tail -n 1)
    compare "$app" "$second"
    compare "$app/Contents/MacOS/tandemd" "$second/Contents/MacOS/tandemd"
    [ $# -eq 4 ] && package "$app" "$4"
    rm -rf "$out"
    ;;
  *) die "usage: dr-check.sh compare|derive|package|all ..." ;;
esac
