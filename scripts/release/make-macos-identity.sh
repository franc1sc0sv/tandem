#!/usr/bin/env bash
# Creates the stable self-signed code-signing identity for the macOS builds (T-348) and prints
# the `gh secret set` commands that store it in the repo. Run it ONCE; the release signs every
# version with it, so the designated requirement (identifier + this certificate) stays the same
# and macOS keeps the Accessibility, Input Monitoring and Local Network grants across updates.
# See docs/dev/release.md#macos-signing-identity.
#
# Usage: scripts/release/make-macos-identity.sh [--out DIR] [--name CN] [--days N] [--force] [--quiet]
#   --out    where to write it (default ~/.config/tandem-release/macos-identity)
#   --name   the certificate's common name (default "Tandem Self-Signed Code Signing")
#   --days   validity (default 7300, 20 years: codesign refuses an expired identity, and a new
#            certificate means a new designated requirement, so every user grants the
#            permissions again once)
#   --force  replace an existing identity in DIR (rotation)
#   --quiet  only write the files (CI uses it for the disposable identity of test mode)
#
# Writes in DIR: identity.p12 (key + certificate), identity.p12.b64 (what the secret holds),
# password (the .p12's random password) and certificate.pem (public). Needs only openssl, so
# it runs on Linux, WSL or macOS. Nothing it writes goes into the repo.
set -euo pipefail

out="${HOME}/.config/tandem-release/macos-identity"
name="Tandem Self-Signed Code Signing"
days=7300
force=false
quiet=false
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out=$2; shift 2 ;;
    --name) name=$2; shift 2 ;;
    --days) days=$2; shift 2 ;;
    --force) force=true; shift ;;
    --quiet) quiet=true; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "make-macos-identity.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$days" in
  ''|*[!0-9]*) echo "make-macos-identity.sh: --days must be a number of days" >&2; exit 2 ;;
esac
command -v openssl >/dev/null || { echo "make-macos-identity.sh: openssl is required" >&2; exit 1; }

if [ -e "$out/identity.p12" ] && [ "$force" != true ]; then
  echo "make-macos-identity.sh: $out/identity.p12 already exists." >&2
  echo "  Replacing it changes the designated requirement: every Mac asks for the permissions again once." >&2
  echo "  To rotate it on purpose, run again with --force (see docs/dev/release.md#rotating-it)." >&2
  exit 1
fi

umask 077
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# No Organization (O) in the subject: with a single self-signed certificate codesign then
# writes the designated requirement as `identifier "..." and certificate leaf = H"<SHA-1>"`.
# The extensions are what codesign needs to accept it as a code-signing identity.
cat >"$work/openssl.cnf" <<EOF
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = $name
[ext]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF

openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days "$days" \
  -config "$work/openssl.cnf" -keyout "$work/key.pem" -out "$work/certificate.pem" 2>/dev/null

openssl rand -base64 33 | tr -d '\n/+=' >"$work/password"
# macOS `security import` does not read the AES/PBKDF2 .p12 that OpenSSL 3 writes by default:
# the classic SHA1-3DES algorithms are readable by both.
openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/certificate.pem" -name "$name" \
  -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 \
  -passout "file:$work/password" -out "$work/identity.p12"
base64 <"$work/identity.p12" | tr -d '\n' >"$work/identity.p12.b64"

for f in identity.p12 identity.p12.b64 password certificate.pem; do
  mv -f "$work/$f" "$out/$f"
done

sha1=$(openssl x509 -in "$out/certificate.pem" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')
until=$(openssl x509 -in "$out/certificate.pem" -noout -enddate | sed 's/^notAfter=//')

if [ "$quiet" = true ]; then
  exit 0
fi

cat <<EOF
Created the macOS signing identity in $out
  Common name: $name
  SHA-1:       $sha1   (it appears in the designated requirement)
  Valid until: $until

Store it as repo secrets (from the repository folder):

  gh secret set MACOS_SIGNING_CERTIFICATE < "$out/identity.p12.b64"
  gh secret set MACOS_SIGNING_CERTIFICATE_PASSWORD < "$out/password"

Keep $out backed up somewhere private (never in the repo): if the
.p12 is lost, the next release needs a new identity and every Mac asks for the
permissions again once.
EOF
