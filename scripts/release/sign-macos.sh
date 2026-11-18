#!/usr/bin/env bash
# Sign and notarize macOS release binaries and the Fleet.app wrapper.
#
# Usage:
#   scripts/release/sign-macos.sh path/to/fleet path/to/fleet-admin \
#     path/to/fleet-daemon path/to/Fleet.app
#
# Directories ending in `.app` are signed as bundles: executables under
# Contents/MacOS are signed first, then the bundle itself. Accepted app
# bundles are stapled so Gatekeeper can verify them offline.
#
# Required environment when signing is enabled:
#   APPLE_CERTIFICATE_P12          base64-encoded Developer ID Application .p12
#   APPLE_CERTIFICATE_PASSWORD     password for the .p12
#   APPLE_TEAM_ID                  Apple Developer team id
#   APPLE_SIGNING_IDENTITY         full identity name (optional; auto-detected)
#
# Notarization is optional and accepts either credential set:
#   App Store Connect API key:
#     APPLE_API_KEY_P8             base64-encoded AuthKey_<KEY_ID>.p8
#     APPLE_API_KEY_ID             App Store Connect key id
#     APPLE_API_ISSUER             App Store Connect issuer id (UUID)
#   Apple ID:
#     APPLE_ID                     Apple ID used for notarization
#     APPLE_APP_SPECIFIC_PASSWORD  app-specific password
#
# If APPLE_CERTIFICATE_P12 is empty the script prints a notice and exits 0, so
# unsigned development releases still work. Set REQUIRE_SIGNING=1 to turn that
# into an error.
set -euo pipefail
umask 077

if [ -z "${APPLE_CERTIFICATE_P12:-}" ]; then
  if [ -n "${REQUIRE_SIGNING:-}" ]; then
    echo "error: REQUIRE_SIGNING is set but APPLE_CERTIFICATE_P12 is empty" >&2
    exit 1
  fi
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
api_key_path="$RUNNER_TEMP/app-signing.p8"
notary_zip="$RUNNER_TEMP/fleet-notarization.zip"

cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -f "$cert_path" "$api_key_path" "$notary_zip"
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

is_app_bundle() {
  [ -d "$1" ] && [ "${1##*.}" = "app" ]
}

sign_file() {
  local path="$1"
  codesign --force --options runtime --timestamp --sign "$identity" "$path"
  codesign --verify --strict --verbose=2 "$path"
}

sign_app() {
  local app="$1" nested main_exec=""
  if [ -f "$app/Contents/Info.plist" ] && command -v plutil >/dev/null 2>&1; then
    main_exec="$(plutil -extract CFBundleExecutable raw -o - \
      "$app/Contents/Info.plist" 2>/dev/null || true)"
  fi
  while IFS= read -r -d '' nested; do
    # codesign treats the bundle's main executable as the whole bundle, so
    # signing it first fails while sibling executables are still unsigned
    # ("code object is not signed at all In subcomponent: ..."). Sign every
    # other nested executable first; signing the bundle below signs the main
    # executable along with the bundle.
    if [ -n "$main_exec" ] && [ "${nested##*/}" = "$main_exec" ]; then
      continue
    fi
    echo "Signing $nested"
    sign_file "$nested"
  done < <(find "$app/Contents/MacOS" -type f -print0 2>/dev/null)
  echo "Signing bundle $app"
  sign_file "$app"
}

app_count=0
for item in "$@"; do
  if is_app_bundle "$item"; then
    sign_app "$item"
    app_count=$((app_count + 1))
  else
    echo "Signing $item"
    sign_file "$item"
  fi
done

# Pick the notarization credential set. An App Store Connect API key wins when
# both are configured; the Apple ID path remains for local runs.
notary_args=()
notary_auth=""
if [ -n "${APPLE_API_KEY_P8:-}" ] && [ -n "${APPLE_API_KEY_ID:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ]; then
  printf '%s' "$APPLE_API_KEY_P8" \
    | python3 -c 'import base64, sys; sys.stdout.buffer.write(base64.b64decode(sys.stdin.read()))' \
    > "$api_key_path"
  chmod 600 "$api_key_path"
  notary_args=(--key "$api_key_path" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER")
  notary_auth="App Store Connect API key $APPLE_API_KEY_ID"
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  notary_args=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD")
  notary_auth="Apple ID $APPLE_ID"
fi

if [ "${#notary_args[@]}" -gt 0 ]; then
  echo "Submitting signed items for notarization with $notary_auth"
  notary_dir="$RUNNER_TEMP/notary"
  mkdir -p "$notary_dir"
  for item in "$@"; do
    if is_app_bundle "$item"; then
      ditto "$item" "$notary_dir/$(basename "$item")"
    else
      cp "$item" "$notary_dir/"
    fi
  done
  (cd "$notary_dir" && ditto -c -k --keepParent . "$notary_zip")
  notary_output="$(xcrun notarytool submit "$notary_zip" \
    "${notary_args[@]}" \
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
      xcrun notarytool log "$notary_id" "${notary_args[@]}" >&2 || true
    fi
    exit 1
  fi
  echo "Notarization accepted"
  if [ "$app_count" -gt 0 ]; then
    for item in "$@"; do
      if is_app_bundle "$item"; then
        echo "Stapling $item"
        xcrun stapler staple "$item"
        xcrun stapler validate "$item"
        codesign --verify --strict --verbose=2 "$item"
      fi
    done
  else
    echo "note: bare command-line binaries are not stapled; Gatekeeper checks the ticket online"
  fi
else
  echo "notice: no notarization credentials configured; items are signed but not notarized"
fi
