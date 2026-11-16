#!/usr/bin/env bash
# Configure the GitHub Actions secrets used by scripts/release/sign-macos.sh.
#
# The script exports the Developer ID Application identity from the login
# keychain, keeps only that identity plus the Developer ID intermediate in a
# temporary .p12, and stores it together with the App Store Connect API key that
# notarization uses as repository secrets. Secret values are never printed.
#
# Usage:
#   scripts/release/configure-signing.sh [options]
#
# Options:
#   --repo OWNER/NAME       Repository (default: the current repository)
#   --identity NAME         Signing identity (default: first Developer ID Application)
#   --p12 PATH              Reuse an existing .p12 export instead of exporting
#                           the keychain; it may contain extra identities
#   --p12-password PASS     Password of the reused .p12 (default: prompt)
#   --api-key PATH          AuthKey_<KEY_ID>.p8 (default: the single key under
#                           ~/.appstoreconnect/private_keys)
#   --key-id ID             App Store Connect key id (default: from the file name)
#   --issuer UUID           App Store Connect issuer id (default: APPLE_API_ISSUER)
#   --dry-run               Report what would be set without calling gh
#   -h, --help              Show this help
#
# Environment:
#   APPLE_API_ISSUER        Issuer id when --issuer is not passed
#   APPLE_P12_PASSWORD      Password for the generated or reused .p12
#
# The key id and issuer id are shown in App Store Connect under
# Users and Access -> Integrations -> App Store Connect API.
set -euo pipefail

usage() {
  sed -n '2,30p' "$0"
}

die() {
  echo "error: $*" >&2
  exit 1
}

repo=""
identity=""
p12_in=""
p12_password="${APPLE_P12_PASSWORD:-}"
api_key=""
key_id=""
issuer="${APPLE_API_ISSUER:-}"
dry_run=0

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) repo="${2:?--repo requires a value}"; shift 2 ;;
    --identity) identity="${2:?--identity requires a value}"; shift 2 ;;
    --p12) p12_in="${2:?--p12 requires a value}"; shift 2 ;;
    --p12-password) p12_password="${2:?--p12-password requires a value}"; shift 2 ;;
    --api-key) api_key="${2:?--api-key requires a value}"; shift 2 ;;
    --key-id) key_id="${2:?--key-id requires a value}"; shift 2 ;;
    --issuer) issuer="${2:?--issuer requires a value}"; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

for tool in security openssl base64; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done
if [ "$dry_run" -eq 0 ]; then
  command -v gh >/dev/null 2>&1 || die "gh is required"
  gh auth status >/dev/null 2>&1 || die "gh is not authenticated; run: gh auth login"
fi

if [ -z "$identity" ]; then
  identity="$(security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(Developer ID Application[^"]*\)".*/\1/p' \
    | head -n 1)"
fi
[ -n "$identity" ] || die "no Developer ID Application identity found; pass --identity"
team_id="$(printf '%s' "$identity" | sed -n 's/.*(\([A-Z0-9]*\))$/\1/p')"
[ -n "$team_id" ] || die "could not read a team id from identity: $identity"

if [ -z "$api_key" ]; then
  shopt -s nullglob
  keys=("$HOME"/.appstoreconnect/private_keys/AuthKey_*.p8)
  shopt -u nullglob
  [ "${#keys[@]}" -eq 1 ] \
    || die "expected one App Store Connect key under ~/.appstoreconnect/private_keys; pass --api-key"
  api_key="${keys[0]}"
fi
[ -f "$api_key" ] || die "API key not found: $api_key"
if [ -z "$key_id" ]; then
  key_id="$(basename "$api_key")"
  key_id="${key_id#AuthKey_}"
  key_id="${key_id%.p8}"
fi
[ -n "$key_id" ] || die "could not read the key id from $api_key; pass --key-id"
[ -n "$issuer" ] || die "pass --issuer or set APPLE_API_ISSUER (App Store Connect -> Users and Access -> Integrations)"

if [ -z "$repo" ]; then
  repo="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
fi
[ -n "$repo" ] || die "could not determine the repository; pass --repo"

login_keychain="$HOME/Library/Keychains/login.keychain-db"
if [ ! -f "$login_keychain" ]; then
  login_keychain="$HOME/Library/Keychains/login.keychain"
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/fleet-signing.XXXXXX")"
chmod 700 "$tmp"
verify_keychain="$tmp/verify.keychain-db"

cleanup() {
  security delete-keychain "$verify_keychain" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT
umask 077

source_p12=""
if [ -n "$p12_in" ]; then
  [ -f "$p12_in" ] || die "p12 not found: $p12_in"
  source_p12="$p12_in"
  if [ -z "$p12_password" ]; then
    read -r -s -p "Password of $p12_in: " p12_password
    echo
  fi
  [ -n "$p12_password" ] || die "empty p12 password"
else
  [ -f "$login_keychain" ] || die "login keychain not found"
  if [ -z "$p12_password" ]; then
    p12_password="$(openssl rand -hex 24)"
  fi
  echo "Exporting identities from the login keychain; approve the macOS prompt." >&2
  security export -k "$login_keychain" -t identities -f pkcs12 \
    -P "$p12_password" -o "$tmp/all.p12"
  source_p12="$tmp/all.p12"
fi

# Pull the leaf certificate from the keychain and keep only the private key
# whose modulus matches it.
security find-certificate -c "$identity" -p > "$tmp/leaf.pem"
[ -s "$tmp/leaf.pem" ] || die "certificate not found in the keychain: $identity"
cert_sha1="$(openssl x509 -in "$tmp/leaf.pem" -noout -fingerprint -sha1 \
  | sed 's/^.*=//' | tr -d ':')"

# OpenSSL 3 keeps the algorithms used by Keychain exports out of the default
# provider, so hand it -legacy. LibreSSL, which older systems ship, has no such
# flag and does not need it.
openssl_major="$(openssl version 2>/dev/null \
  | sed -n 's/^OpenSSL \([0-9][0-9]*\).*/\1/p')"
pkcs12() {
  if [ -n "$openssl_major" ] && [ "$openssl_major" -ge 3 ]; then
    openssl pkcs12 -legacy "$@"
  else
    openssl pkcs12 "$@"
  fi
}

pkcs12 -in "$source_p12" -nodes -passin "pass:$p12_password" -out "$tmp/all.pem"
[ -s "$tmp/all.pem" ] || die "could not read $source_p12; wrong password?"

awk -v dir="$tmp" '
  /-----BEGIN (RSA )?PRIVATE KEY-----/ { n++; file = dir "/key-" n ".pem" }
  n > 0 { print > file }
' "$tmp/all.pem"
[ -n "$(ls "$tmp"/key-*.pem 2>/dev/null)" ] \
  || die "no private keys found in $source_p12"

leaf_modulus="$(openssl x509 -in "$tmp/leaf.pem" -noout -modulus | sed 's/^Modulus=//')"
matched_key=""
for key in "$tmp"/key-*.pem; do
  key_modulus="$(openssl rsa -in "$key" -noout -modulus 2>/dev/null \
    | sed 's/^Modulus=//' || true)"
  if [ -n "$key_modulus" ] && [ "$key_modulus" = "$leaf_modulus" ]; then
    matched_key="$key"
    break
  fi
done
[ -n "$matched_key" ] || die "$identity has no private key in $source_p12"

pkcs12 -export \
  -inkey "$matched_key" \
  -in "$tmp/leaf.pem" \
  -name "$identity" \
  -passout "pass:$p12_password" \
  -out "$tmp/developer-id.p12"

security create-keychain -p verify "$verify_keychain"
security unlock-keychain -p verify "$verify_keychain"
security import "$tmp/developer-id.p12" -k "$verify_keychain" \
  -P "$p12_password" -T /usr/bin/codesign -T /usr/bin/security
if ! security find-identity -v -p codesigning "$verify_keychain" | grep -q "$identity"; then
  die "the generated p12 does not contain a usable $identity identity"
fi
security delete-keychain "$verify_keychain"

cat >&2 <<EOF
repository:  $repo
identity:    $identity
team id:     $team_id
cert sha1:   $cert_sha1
notary key:  $key_id ($api_key)
issuer id:   $issuer
EOF

if [ "$dry_run" -eq 1 ]; then
  echo "dry run: no secrets were changed" >&2
  exit 0
fi

base64 < "$tmp/developer-id.p12" | tr -d '\n' \
  | gh secret set APPLE_CERTIFICATE_P12 --repo "$repo"
printf '%s' "$p12_password" \
  | gh secret set APPLE_CERTIFICATE_PASSWORD --repo "$repo"
printf '%s' "$identity" \
  | gh secret set APPLE_SIGNING_IDENTITY --repo "$repo"
printf '%s' "$team_id" \
  | gh secret set APPLE_TEAM_ID --repo "$repo"
base64 < "$api_key" | tr -d '\n' \
  | gh secret set APPLE_API_KEY_P8 --repo "$repo"
printf '%s' "$key_id" \
  | gh secret set APPLE_API_KEY_ID --repo "$repo"
printf '%s' "$issuer" \
  | gh secret set APPLE_API_ISSUER --repo "$repo"

echo "Secrets written to $repo:" >&2
gh secret list --repo "$repo" | sed 's/^/  /' >&2
