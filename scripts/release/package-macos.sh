#!/usr/bin/env bash
# Assemble the macOS release layout: bin/, web/, and the Fleet.app wrapper.
#
# The app bundle gives launchd jobs a display name. Without it, System Settings
# -> General -> Login Items & Extensions falls back to the Developer ID
# certificate name (the signer's personal or organization name) instead of
# "Fleet".
#
# Usage:
#   scripts/release/package-macos.sh --version 0.2.2 \
#     --bin-dir target/release --web-dir web-dist --output-dir pkg
#
# Options:
#   --version VERSION    Release version, with or without a leading `v`
#                        (default: the workspace package version)
#   --bin-dir DIR        Directory containing fleet, fleet-admin, fleet-daemon
#                        (default: target/release)
#   --web-dir DIR        Directory containing the built web UI
#                        (default: apps/web/dist)
#   --output-dir DIR     Destination directory (default: dist)
#   --bundle-id ID       CFBundleIdentifier (default: dev.fleet.app)
#   --no-app             Emit only bin/ and web/, without Fleet.app
#   -h, --help           Show this help
#
# The output is ready for scripts/release/sign-macos.sh and for packing:
#
#   tar -czf fleet-darwin-arm64.tar.gz -C pkg .
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

version=""
bin_dir="$root/target/release"
web_dir="$root/apps/web/dist"
output_dir="$root/dist"
bundle_id="dev.fleet.app"
make_app=1
app_name="Fleet.app"
app_executable="fleet"

usage() {
  sed -n '2,26p' "$0"
}

die() {
  echo "error: $*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="${2:?--version requires a value}"; shift 2 ;;
    --bin-dir) bin_dir="${2:?--bin-dir requires a value}"; shift 2 ;;
    --web-dir) web_dir="${2:?--web-dir requires a value}"; shift 2 ;;
    --output-dir) output_dir="${2:?--output-dir requires a value}"; shift 2 ;;
    --bundle-id) bundle_id="${2:?--bundle-id requires a value}"; shift 2 ;;
    --no-app) make_app=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

if [ -z "$version" ]; then
  version="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' \
    "$root/Cargo.toml" | head -n 1)"
fi
[ -n "$version" ] || die "could not read the workspace version; pass --version"
version="${version#v}"
# CFBundleShortVersionString and CFBundleVersion accept at most three
# period-separated integers. A prerelease suffix is not part of the bundle
# version.
bundle_version="${version%%-*}"
bundle_version="${bundle_version%%+*}"
case "$bundle_version" in
  ""|.*|*.|*..*|*[!0-9.]*) die "invalid version for a macOS bundle: $version" ;;
esac
if [ "$(printf '%s' "$bundle_version" | awk -F. '{print NF}')" -gt 3 ]; then
  die "macOS bundle versions have at most three components: $bundle_version"
fi

case "$bundle_id" in
  *[!A-Za-z0-9.-]*|.*|*.) die "invalid bundle identifier: $bundle_id" ;;
esac
case "$bundle_id" in
  *.*) ;;
  *) die "bundle identifier must be reverse-DNS: $bundle_id" ;;
esac

[ -d "$bin_dir" ] || die "binary directory not found: $bin_dir"
[ -d "$web_dir" ] || die "web directory not found: $web_dir"
for binary in fleet fleet-admin fleet-daemon; do
  [ -f "$bin_dir/$binary" ] || die "missing $bin_dir/$binary"
done
[ -f "$web_dir/index.html" ] || die "missing $web_dir/index.html"

mkdir -p "$output_dir"
out="$(cd "$output_dir" && pwd)"
[ "$out" != "/" ] || die "refusing to package into /"

rm -rf "$out/bin" "$out/web" "$out/$app_name"
install -d -m 0755 "$out/bin" "$out/web"
install -m 0755 "$bin_dir/fleet" "$out/bin/fleet"
install -m 0755 "$bin_dir/fleet-admin" "$out/bin/fleet-admin"
install -m 0755 "$bin_dir/fleet-daemon" "$out/bin/fleet-daemon"
cp -R "$web_dir/." "$out/web/"

if [ "$make_app" -eq 1 ]; then
  template="$root/packaging/macos/Info.plist.in"
  [ -f "$template" ] || die "missing app template: $template"
  app="$out/$app_name"
  install -d -m 0755 "$app/Contents/MacOS"
  install -m 0755 "$bin_dir/fleet" "$app/Contents/MacOS/fleet"
  install -m 0755 "$bin_dir/fleet-admin" "$app/Contents/MacOS/fleet-admin"
  install -m 0755 "$bin_dir/fleet-daemon" "$app/Contents/MacOS/fleet-daemon"
  sed \
    -e "s|@BUNDLE_ID@|$bundle_id|g" \
    -e "s|@BUNDLE_EXECUTABLE@|$app_executable|g" \
    -e "s|@VERSION@|$bundle_version|g" \
    -e "s|@BUILD_VERSION@|$bundle_version|g" \
    "$template" >"$app/Contents/Info.plist"
  chmod 0644 "$app/Contents/Info.plist"
  if command -v plutil >/dev/null 2>&1; then
    plutil -lint "$app/Contents/Info.plist" >/dev/null
  fi
fi

echo "Packaged macOS layout in $out"
echo "  binaries  $out/bin"
echo "  web       $out/web"
if [ "$make_app" -eq 1 ]; then
  echo "  app       $out/$app_name"
fi
