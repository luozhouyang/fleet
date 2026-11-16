#!/usr/bin/env bash
# Sign and notarize macOS release binaries.
#
# Usage:
#   scripts/release/sign-macos.sh path/to/fleet path/to/fleet-admin path/to/fleet-daemon
#
# Required environment when signing is enabled:
#   APPLE_CERTIFICATE_P12          base64-encoded Developer ID Application .p12
#   APPLE_CERTIFICATE_PASSWORD     password for the .p12
#   APPLE_TEAM_ID                  Apple Developer team id
#   APPLE_ID                       Apple ID used for notarization (optional)
#   APPLE_APP_SPECIFIC_PASSWORD    app-specific password (optional)
#   APPLE_SIGNING_IDENTITY         full identity name (optional; auto-detected)
#
# If APPLE_CERTIFICATE_P12 is empty the script prints a notice and exits 0, so
# unsigned development releases still work.
set -euo pipefail
umask 077

if [ -z "${APPLE_CERTIFICATE_P12:-}" ]; then
  echo "notice: APPLE_CERTIFICATE_P12 is not set; skipping macOS signing and notarization"
  exit 0
fi

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <binary> [binary...]" >&2
  exit 2
fi

for tool in codesign security xcrun; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "error: $tool is required for macOS signing" >&2
    exit 1
  }
done

keychain="$RUNNER_TEMP/app-signing.keychain-db"
keychain_password="$(openssl rand -hex 24)"
cert_path="$RUNNER_TEMP/app-signing.p12"
notary_zip="$RUNNER_TEMP/fleet-notarization.zip"

cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -f "$cert_path" "$notary_zip"
}
trap cleanup EXIT

security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security default-keychain -s "$keychain"
security list-keychains -d user -s "$keychain" \
  $(security list-keychains -d user | tr -d '"')

printf '%s' "$APPLE_CERTIFICATE_P12" \
  | python3 -c 'import base64, sys; sys.stdout.buffer.write(base64.b64decode(sys.stdin.read()))' \
  > "$cert_path"

security import "$cert_path" \
  -k "$keychain" \
  -P "${APPLE_CERTIFICATE_PASSWORD:?APPLE_CERTIFICATE_PASSWORD is required}" \
  -T /usr/bin/codesign \
  -T /usr/bin/security

security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s -k "$keychain_password" "$keychain" >/dev/null

identity="${APPLE_SIGNING_IDENTITY:-}"
if [ -z "$identity" ]; then
  identity="$(security find-identity -v -p codesigning "$keychain" \
    | sed -n 's/.*"\(Developer ID Application[^"]*\)".*/\1/p' \
    | head -n 1)"
fi
if [ -z "$identity" ]; then
  echo "error: no Developer ID Application identity found in the imported certificate" >&2
  exit 1
fi

echo "Signing with: $identity"
for binary in "$@"; do
  codesign --force --options runtime --timestamp --sign "$identity" "$binary"
  codesign --verify --strict --verbose=2 "$binary"
done

if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  echo "Submitting signed binaries for notarization"
  notary_dir="$RUNNER_TEMP/notary"
  mkdir -p "$notary_dir"
  for binary in "$@"; do
    cp "$binary" "$notary_dir/"
  done
  (cd "$notary_dir" && ditto -c -k --keepParent . "$notary_zip")
  notary_output="$(xcrun notarytool submit "$notary_zip" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait \
    --output-format json)"
  printf '%s\n' "$notary_output"
  notary_status="$(printf '%s' "$notary_output" \
    | python3 -c 'import json, sys; print(json.load(sys.stdin).get("status", ""))')"
  if [ "$notary_status" != "Accepted" ]; then
    echo "error: notarization finished with status: ${notary_status:-unknown}" >&2
    notary_id="$(printf '%s' "$notary_output" \
      | python3 -c 'import json, sys; print(json.load(sys.stdin).get("id", ""))')"
    if [ -n "$notary_id" ]; then
      echo "Fetching the notarization log for submission $notary_id" >&2
      xcrun notarytool log "$notary_id" \
        --apple-id "$APPLE_ID" \
        --team-id "$APPLE_TEAM_ID" \
        --password "$APPLE_APP_SPECIFIC_PASSWORD" >&2 || true
    fi
    exit 1
  fi
  echo "Notarization accepted"
  echo "note: bare command-line binaries are not stapled; Gatekeeper checks the ticket online"
else
  echo "notice: notarization credentials are incomplete; binaries are signed but not submitted"
fi
