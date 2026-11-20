#!/usr/bin/env bash
# Tests for version.sh, notes.sh, latest-json.sh, make-macos-identity.sh and
# ../macos/dr-check.sh (with a fake codesign) on throwaway dirs and git repos. They run in the `prepare` job
# of the release workflow and in the Ubuntu CI. Usage: scripts/release/test.sh
# shellcheck disable=SC2015,SC2016  # `ok`/`bad` never fail; the backticks are literal Markdown.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# Hermetic test repos: no global or system config from whoever runs this.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.com

failures=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1" >&2; failures=$((failures + 1)); }
contains() { # name, text, pattern (literal)
  if grep -qF -- "$3" <<<"$2"; then ok "$1"; else bad "$1: missing "$3""; echo "$2" >&2; fi
}
not_contains() {
  if grep -qF -- "$3" <<<"$2"; then bad "$1: unexpected "$3""; echo "$2" >&2; else ok "$1"; fi
}

# --- version.sh -------------------------------------------------------------------------
versions() { # dir, cargo, tauri, npm
  mkdir -p "$1/apps/tandem-ui/src-tauri"
  printf '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "%s"\nedition = "2024"\n\n[workspace.dependencies]\nfoo = { version = "9.9.9" }\n' "$2" >"$1/Cargo.toml"
  printf '{ "productName": "Tandem", "version": "%s" }\n' "$3" >"$1/apps/tandem-ui/src-tauri/tauri.conf.json"
  printf '{ "name": "tandem-ui", "version": "%s" }\n' "$4" >"$1/apps/tandem-ui/package.json"
}

versions "$tmp/v1" 1.2.3 1.2.3 1.2.3
[ "$("$here/version.sh" --root "$tmp/v1")" = 1.2.3 ] && ok "version: reads the workspace one" || bad "version: reads the workspace one"
"$here/version.sh" --root "$tmp/v1" v1.2.3 >/dev/null && ok "version: accepts a matching tag" || bad "version: accepts a matching tag"
if "$here/version.sh" --root "$tmp/v1" v1.2.4 >/dev/null 2>&1; then bad "version: rejects a different tag"; else ok "version: rejects a different tag"; fi
if "$here/version.sh" --root "$tmp/v1" 1.2.3 >/dev/null 2>&1; then bad "version: requires the v in the tag"; else ok "version: requires the v in the tag"; fi
versions "$tmp/v2" 1.2.3 1.2.4 1.2.3
if "$here/version.sh" --root "$tmp/v2" >/dev/null 2>&1; then bad "version: rejects a mismatched tauri"; else ok "version: rejects a mismatched tauri"; fi
versions "$tmp/v3" 1.2.3 1.2.3 0.0.1
if "$here/version.sh" --root "$tmp/v3" >/dev/null 2>&1; then bad "version: rejects a mismatched package.json"; else ok "version: rejects a mismatched package.json"; fi

# --- notes.sh ---------------------------------------------------------------------------
repo="$tmp/repo"
git init -q -b develop "$repo"
cd "$repo"
task() { # id, phase, pending checkboxes
  mkdir -p docs/tasks
  {
    printf -- '---\nid: %s\ntitle: "x"\nphase: %s\nstatus: done\n---\n\n# %s\n\n## Acceptance criteria\n\n- [ ] does not count\n' "$1" "$2" "$1"
    if [ "$3" -gt 0 ]; then
      printf '\n## Pending hardware verification\n\n'
      for _ in $(seq "$3"); do printf -- '- [ ] something\n'; done
      printf -- '- [x] already done\n\n## Notes\n\n- [ ] does not count either\n'
    fi
  } >"docs/tasks/$1.md"
}
commit() { git add -A && git commit -q --allow-empty -m "$1"; }

task T-001 F1 0; commit "T-001: Workspace skeleton (#1)"
commit "Initial design"
task T-040 F5 2; commit "T-040: UI skeleton (#2)"
task T-010 F3 0; commit "T-010: DDC/CI on Windows (dxva2) (#3)"
git tag v0.1.0
task T-070 F5 1; commit "T-070: macOS installer (#4)"
commit "T-070: fix the signature (#5)"
task T-099 F9 0; commit "T-099: task in an odd phase (#6)"
commit "T-098: task without a file"
git switch -q -c other && commit "Separate branch" && git switch -q develop
git merge -q --no-ff -m "Merge branch 'other'" other
commit "Backlog: more tasks (#7)"
git tag v0.2.0

all=$("$here/notes.sh" --version 0.1.0 --to v0.1.0)
contains "notes: title with version" "$all" "# Tandem 0.1.0"
contains "notes: first release without a previous tag" "$all" "First release: 3 tasks merged."
contains "notes: task with its PR" "$all" "- **T-001** Workspace skeleton (#1)"
contains "notes: phase from the frontmatter" "$all" "## F3 Monitors and modes"
contains "notes: other changes" "$all" "- Initial design"
contains "notes: pending hardware counts only its section" "$all" "- **T-040** UI skeleton: 2"
not_contains "notes: no later tasks" "$all" "T-070"
# Phases come out in the design order (F1 before F3 before F5).
phase_order=$(grep '^## F' <<<"$all" | tr '\n' '|')
[ "$phase_order" = "## F1 Core|## F3 Monitors and modes|## F5 UI|" ] && ok "notes: phase order" || bad "notes: phase order ($phase_order)"

second=$("$here/notes.sh" --to v0.2.0)
contains "notes: since the previous tag" "$second" 'Changes since `v0.1.0`: 3 tasks merged.'
not_contains "notes: excludes the previous release" "$second" "T-001"
contains "notes: a task with several PRs appears once" "$second" "- **T-070** macOS installer (#4, #5)"
contains "notes: task without a file goes under no phase" "$second" "## No phase"
contains "notes: task without a PR" "$second" "- **T-098** task without a file"
contains "notes: unknown phase as is" "$second" "## F9"
contains "notes: commits from a merged branch" "$second" "- Separate branch"
not_contains "notes: no merge commits" "$second" "Merge branch"
[ "$(grep -c 'T-070' <<<"$second")" = 2 ] && ok "notes: T-070 in its phase and in hardware" || bad "notes: T-070 repeated"
last=$(grep '^## ' <<<"$second" | tail -n 1)
[ "$last" = "## Pending hardware verification" ] && ok "notes: hardware last" || bad "notes: hardware last ($last)"
f9=$(grep -n '^## F9' <<<"$second" | cut -d: -f1); np=$(grep -n '^## No phase' <<<"$second" | cut -d: -f1)
[ "$f9" -lt "$np" ] && ok "notes: unknown phases last" || bad "notes: unknown phases last"

range=$("$here/notes.sh" --from v0.1.0 --to HEAD~1)
not_contains "notes: --from y --to explicit" "$range" "Backlog: more tasks"
if "$here/notes.sh" --to does-not-exist >/dev/null 2>&1; then bad "notes: nonexistent ref"; else ok "notes: nonexistent ref"; fi

# --- latest-json.sh ---------------------------------------------------------------------
cd "$tmp"
mkdir packages
printf 'app' >packages/Tandem_0.3.1_aarch64.app.tar.gz
printf 'mac-signature\n' >packages/Tandem_0.3.1_aarch64.app.tar.gz.sig
printf 'exe' >packages/Tandem_0.3.1_x64-setup.exe
printf 'win-signature' >packages/Tandem_0.3.1_x64-setup.exe.sig
printf '# Tandem 0.3.1\n\n- something\n' >notes-latest.md
latest=$("$here/latest-json.sh" --version 0.3.1 --repo owner/tandem --tag v0.3.1 \
  --notes notes-latest.md --date 2026-11-17T12:00:00Z \
  darwin-aarch64=packages/Tandem_0.3.1_aarch64.app.tar.gz \
  windows-x86_64=packages/Tandem_0.3.1_x64-setup.exe)
expected='{"version":"0.3.1","notes":"# Tandem 0.3.1\n\n- something","pub_date":"2026-11-17T12:00:00Z","platforms":{"darwin-aarch64":{"signature":"mac-signature","url":"https://github.com/owner/tandem/releases/download/v0.3.1/Tandem_0.3.1_aarch64.app.tar.gz"},"windows-x86_64":{"signature":"win-signature","url":"https://github.com/owner/tandem/releases/download/v0.3.1/Tandem_0.3.1_x64-setup.exe"}}}'
[ "$(jq -cS . <<<"$latest")" = "$(jq -cS . <<<"$expected")" ] && ok "latest: updater shape" || { bad "latest: updater shape"; echo "$latest" >&2; }
rejects() { # name, arguments...
  local name=$1; shift
  if "$here/latest-json.sh" "$@" >/dev/null 2>&1; then bad "latest: $name"; else ok "latest: $name"; fi
}
base=(--version 0.3.1 --repo owner/tandem --tag v0.3.1)
rm packages/Tandem_0.3.1_x64-setup.exe.sig
rejects "requires the signature" "${base[@]}" windows-x86_64=packages/Tandem_0.3.1_x64-setup.exe
: >packages/Tandem_0.3.1_x64-setup.exe.sig
rejects "empty signature" "${base[@]}" windows-x86_64=packages/Tandem_0.3.1_x64-setup.exe
rejects "unknown platform" "${base[@]}" windows-arm=packages/Tandem_0.3.1_aarch64.app.tar.gz
rejects "repeated platform" "${base[@]}" darwin-aarch64=packages/Tandem_0.3.1_aarch64.app.tar.gz darwin-aarch64=packages/Tandem_0.3.1_aarch64.app.tar.gz
rejects "nonexistent package" "${base[@]}" darwin-aarch64=packages/does-not-exist
rejects "no packages" "${base[@]}"
rejects "no version" --repo owner/tandem --tag v0.3.1 darwin-aarch64=packages/Tandem_0.3.1_aarch64.app.tar.gz

# --- make-macos-identity.sh (T-348) -------------------------------------------------------
idout="$tmp/identity"
idmsg=$("$here/make-macos-identity.sh" --out "$idout" --days 30)
for f in identity.p12 identity.p12.b64 password certificate.pem; do
  [ -s "$idout/$f" ] && ok "identity: writes $f" || bad "identity: writes $f"
done
contains "identity: prints the certificate secret command" "$idmsg" "gh secret set MACOS_SIGNING_CERTIFICATE < \"$idout/identity.p12.b64\""
contains "identity: prints the password secret command" "$idmsg" "gh secret set MACOS_SIGNING_CERTIFICATE_PASSWORD < \"$idout/password\""
not_contains "identity: never prints the password" "$idmsg" "$(cat "$idout/password")"
[ "$(stat -c %a "$idout/identity.p12")" = 600 ] && ok "identity: private files" || bad "identity: private files"
certpem=$(openssl pkcs12 -in "$idout/identity.p12" -passin "file:$idout/password" -nokeys 2>/dev/null)
certtext=$(openssl x509 -noout -text <<<"$certpem")
# RFC 2253 prints the subject the same way in every OpenSSL version.
subject=$(openssl x509 -noout -subject -nameopt RFC2253 <<<"$certpem")
[ "$subject" = "subject=CN=Tandem Self-Signed Code Signing" ] \
  && ok "identity: the .p12 opens with its password and carries the certificate" \
  || bad "identity: the .p12 opens with its password and carries the certificate ($subject)"
contains "identity: code-signing usage" "$certtext" "Code Signing"
contains "identity: not a CA" "$certtext" "CA:FALSE"
not_contains "identity: no Organization (the DR pins the leaf)" "$subject" "O="
openssl pkcs12 -in "$idout/identity.p12" -passin "file:$idout/password" -nocerts -nodes 2>/dev/null | grep -q "PRIVATE KEY" \
  && ok "identity: the .p12 carries the private key" || bad "identity: the .p12 carries the private key"
[ "$(base64 -d <"$idout/identity.p12.b64" | cmp - "$idout/identity.p12" && echo same)" = same ] \
  && ok "identity: the secret is the .p12 in base64" || bad "identity: the secret is the .p12 in base64"
before=$(sha256sum "$idout/identity.p12")
if "$here/make-macos-identity.sh" --out "$idout" >/dev/null 2>&1; then bad "identity: refuses to overwrite"; else ok "identity: refuses to overwrite"; fi
[ "$(sha256sum "$idout/identity.p12")" = "$before" ] && ok "identity: the old one is kept" || bad "identity: the old one is kept"
"$here/make-macos-identity.sh" --out "$idout" --force --quiet >/dev/null
[ "$(sha256sum "$idout/identity.p12")" != "$before" ] && ok "identity: --force rotates it" || bad "identity: --force rotates it"
if "$here/make-macos-identity.sh" --out "$tmp/id2" --days soon >/dev/null 2>&1; then bad "identity: rejects bad --days"; else ok "identity: rejects bad --days"; fi

# --- ../macos/dr-check.sh (T-348), with a fake codesign --------------------------------------
# The fake reads `<path>/.dr` and `<path>/.cdhash` (or `<file>.dr` for a bare binary) and, for
# `--verify -R=<req>`, succeeds only if the requirement is that DR.
drcheck="$here/../macos/dr-check.sh"
fake="$tmp/fakebin"
mkdir -p "$fake"
cat >"$fake/codesign" <<'FAKE'
#!/usr/bin/env bash
path=${@: -1}
meta() { if [ -d "$path" ]; then cat "$path/.$1" 2>/dev/null; else cat "$path.$1" 2>/dev/null; fi; }
case "$*" in
  "-d -r- "*) d=$(meta dr); [ -n "$d" ] && echo "designated => $d"; echo "Executable=$path" >&2 ;;
  "-d -vvv "*) echo "CDHash=$(meta cdhash)" >&2 ;;
  "--verify --strict -R ="*) req=${4#=}; [ "$req" = "$(meta dr)" ] ;;
  "--verify --deep --strict "*) [ -n "$(meta dr)" ] ;;
  *) echo "fake codesign: unexpected $*" >&2; exit 2 ;;
esac
FAKE
chmod +x "$fake/codesign"
build() { # dir, dr, cdhash
  mkdir -p "$1"
  printf '%s' "$2" >"$1/.dr"
  printf '%s' "$3" >"$1/.cdhash"
}
stable='identifier "com.franc1sc0sv.tandem" and certificate leaf = H"c93142ce4dd047bff26f7522754dfcecf7170200"'
build "$tmp/dr/a/Tandem.app" "$stable" aaaa
build "$tmp/dr/b/Tandem.app" "$stable" bbbb
drrun() { PATH="$fake:$PATH" "$drcheck" "$@" 2>&1; }
out=$(drrun compare "$tmp/dr/a/Tandem.app" "$tmp/dr/b/Tandem.app") \
  && contains "dr: same DR, different builds passes" "$out" "ok: same designated requirement" \
  || { bad "dr: same DR, different builds passes"; echo "$out" >&2; }
build "$tmp/dr/c/Tandem.app" "${stable/c931/d931}" cccc
out=$(drrun compare "$tmp/dr/a/Tandem.app" "$tmp/dr/c/Tandem.app") && bad "dr: another certificate fails" \
  || contains "dr: another certificate fails" "$out" "the designated requirements differ"
build "$tmp/dr/d/Tandem.app" 'cdhash H"1111"' dddd
build "$tmp/dr/e/Tandem.app" 'cdhash H"2222"' eeee
out=$(drrun compare "$tmp/dr/d/Tandem.app" "$tmp/dr/e/Tandem.app") && bad "dr: ad hoc fails" \
  || contains "dr: ad hoc fails" "$out" "signed ad hoc"
out=$(drrun compare "$tmp/dr/a/Tandem.app" "$tmp/dr/a/Tandem.app") && bad "dr: the same build proves nothing" \
  || contains "dr: the same build proves nothing" "$out" "same cdhash"
build "$tmp/dr/f/Tandem.app" "" ffff
out=$(drrun compare "$tmp/dr/a/Tandem.app" "$tmp/dr/f/Tandem.app") && bad "dr: unsigned fails" \
  || contains "dr: unsigned fails" "$out" "no designated requirement"
# Bare binaries (tandemd).
printf x >"$tmp/dr/tandemd-a"; printf 'identifier "tandemd" and certificate leaf = H"c931"' >"$tmp/dr/tandemd-a.dr"; printf 1 >"$tmp/dr/tandemd-a.cdhash"
printf y >"$tmp/dr/tandemd-b"; cp "$tmp/dr/tandemd-a.dr" "$tmp/dr/tandemd-b.dr"; printf 2 >"$tmp/dr/tandemd-b.cdhash"
drrun compare "$tmp/dr/tandemd-a" "$tmp/dr/tandemd-b" >/dev/null && ok "dr: bare binaries" || bad "dr: bare binaries"
# The updater package keeps the DR of the app and of its tandemd.
pkgapp="$tmp/dr/a/Tandem.app"
mkdir -p "$pkgapp/Contents/MacOS"
printf x >"$pkgapp/Contents/MacOS/tandemd"; cp "$tmp/dr/tandemd-a.dr" "$pkgapp/Contents/MacOS/tandemd.dr"
tar -czf "$tmp/dr/ok.app.tar.gz" -C "$tmp/dr/a" Tandem.app
out=$(drrun package "$pkgapp" "$tmp/dr/ok.app.tar.gz") && contains "dr: the updater package keeps the DR" "$out" "ok: the updater package" \
  || { bad "dr: the updater package keeps the DR"; echo "$out" >&2; }
mkdir -p "$tmp/dr/other" && cp -R "$tmp/dr/c/Tandem.app" "$tmp/dr/other/" && mkdir -p "$tmp/dr/other/Tandem.app/Contents/MacOS"
printf x >"$tmp/dr/other/Tandem.app/Contents/MacOS/tandemd"; cp "$tmp/dr/tandemd-a.dr" "$tmp/dr/other/Tandem.app/Contents/MacOS/tandemd.dr"
tar -czf "$tmp/dr/other.app.tar.gz" -C "$tmp/dr/other" Tandem.app
out=$(drrun package "$pkgapp" "$tmp/dr/other.app.tar.gz") && bad "dr: an updater package with another identity fails" \
  || contains "dr: an updater package with another identity fails" "$out" "another designated requirement"

if [ "$failures" -gt 0 ]; then echo "$failures tests failed" >&2; exit 1; fi
echo "all tests passed"
