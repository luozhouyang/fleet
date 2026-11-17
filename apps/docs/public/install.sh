#!/bin/sh
# Fleet installer for Linux and macOS.
#
# One-line install:
#   curl -fsSL https://luozhouyang.github.io/fleet/install.sh | sh
#
# Options:
#   --version VERSION   Release to install (default: latest)
#   --prefix DIR        Install prefix (default: ~/.local)
#   --port PORT         Admin loopback port (default: 8787)
#   --yes               Never prompt for sudo; requires passwordless sudo
#   --no-service        Copy files only (tests/debugging)
#   --no-app            macOS: skip the Fleet.app wrapper
#   --app-dir DIR       macOS: Fleet.app location (default: ~/Applications)
#   --force             Reinstall over an existing prefix
#   --dry-run           Print the plan and exit
#
# Environment:
#   FLEET_VERSION, FLEET_PREFIX, FLEET_BASE_URL
#
# The script only downloads and verifies the release archive. All installation
# logic lives in `fleet install`, which writes user files without elevation and
# asks for sudo once to install the system service definitions.

set -eu

REPO="luozhouyang/fleet"
VERSION="${FLEET_VERSION:-latest}"
PREFIX="${FLEET_PREFIX:-}"
PORT=""
DRY_RUN=0
FORCE=0
YES=0
NO_SERVICE=0
INSTALL_APP=1
APP_DIR=""

usage() {
  cat <<'EOF'
Fleet installer for Linux and macOS.

Usage:
  curl -fsSL https://luozhouyang.github.io/fleet/install.sh | sh
  sh install.sh [options]

Options:
  --version VERSION   Release to install (default: latest)
  --prefix DIR        Install prefix (default: ~/.local)
  --port PORT         Admin loopback port (default: 8787)
  --yes               Never prompt for sudo; requires passwordless sudo
  --no-service        Copy files only (tests/debugging)
  --no-app            macOS: skip the Fleet.app wrapper
  --app-dir DIR       macOS: Fleet.app directory (default: ~/Applications)
  --force             Reinstall over an existing prefix
  --dry-run           Print the plan and exit

Docs: https://luozhouyang.github.io/fleet/getting-started/installation/
EOF
}

log() {
  printf '%s\n' "$*"
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || die "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --prefix)
      [ $# -ge 2 ] || die "--prefix requires a value"
      PREFIX="$2"
      shift 2
      ;;
    --port)
      [ $# -ge 2 ] || die "--port requires a value"
      PORT="$2"
      shift 2
      ;;
    --yes)
      YES=1
      shift
      ;;
    --no-service)
      NO_SERVICE=1
      shift
      ;;
    --no-app)
      INSTALL_APP=0
      shift
      ;;
    --app-dir)
      [ $# -ge 2 ] || die "--app-dir requires a value"
      APP_DIR="$2"
      shift 2
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1 (try --help)"
      ;;
  esac
done

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$os" in
  linux) ;;
  darwin) ;;
  *) die "unsupported operating system: $os (on Windows, use install.ps1)" ;;
esac

machine="$(uname -m)"
case "$machine" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="arm64" ;;
  *) die "unsupported architecture: $machine" ;;
esac

asset="fleet-$os-$arch.tar.gz"

if [ -z "$PREFIX" ]; then
  PREFIX="$HOME/.local"
fi

case "$VERSION" in
  latest)
    base="https://github.com/$REPO/releases/latest/download"
    ;;
  v*)
    base="https://github.com/$REPO/releases/download/$VERSION"
    ;;
  *)
    base="https://github.com/$REPO/releases/download/v$VERSION"
    ;;
esac
if [ -n "${FLEET_BASE_URL:-}" ]; then
  base="${FLEET_BASE_URL%/}"
fi

if [ -n "$APP_DIR" ]; then
  case "$APP_DIR" in
    /*) ;;
    *) die "--app-dir must be an absolute path: $APP_DIR" ;;
  esac
  [ "$os" = "darwin" ] || die "--app-dir is supported on macOS only"
fi
if [ "$INSTALL_APP" -eq 0 ] && [ "$os" != "darwin" ]; then
  die "--no-app is supported on macOS only"
fi

app_dest=""
if [ "$os" = "darwin" ] && [ "$INSTALL_APP" -eq 1 ]; then
  if [ -n "$APP_DIR" ]; then
    app_dest="$APP_DIR/Fleet.app"
  else
    app_dest="$HOME/Applications/Fleet.app"
  fi
fi

log "Fleet installer"
log "  platform: $os/$arch"
log "  release:  $VERSION"
log "  prefix:   $PREFIX"
if [ -n "$PORT" ]; then
  log "  port:     $PORT"
fi
if [ -n "$app_dest" ]; then
  log "  app:      $app_dest"
fi
log "  source:   $base/$asset"

if [ "$DRY_RUN" -eq 1 ]; then
  log "dry run: nothing was downloaded or written"
  exit 0
fi

if [ "$(id -u)" -eq 0 ]; then
  die "run the installer as your normal user, not root.
Fleet keeps configuration and state in your home directory and asks for
elevation once, only to install the system service definitions."
fi

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t fleet)"
trap 'rm -rf "$tmp"' EXIT INT TERM HUP

download() {
  curl -fL --retry 3 --connect-timeout 10 --silent --show-error "$1" -o "$2"
}

log "Downloading $asset"
download "$base/$asset" "$tmp/$asset" || die "download failed: $base/$asset"
download "$base/SHA256SUMS" "$tmp/SHA256SUMS" 2>/dev/null || true

if [ -s "$tmp/SHA256SUMS" ]; then
  expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "$tmp/SHA256SUMS")"
  if [ -n "$expected" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      actual="$(sha256sum "$tmp/$asset" | awk '{ print $1 }')"
    elif command -v shasum >/dev/null 2>&1; then
      actual="$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')"
    else
      actual=""
    fi
    if [ -n "$actual" ] && [ "$expected" != "$actual" ]; then
      die "checksum mismatch for $asset"
    fi
  fi
fi

mkdir -p "$tmp/unpack"
tar -xzf "$tmp/$asset" -C "$tmp/unpack"
[ -f "$tmp/unpack/bin/fleet" ] || die "archive does not contain bin/fleet"
[ -f "$tmp/unpack/bin/fleet-admin" ] || die "archive does not contain bin/fleet-admin"
[ -f "$tmp/unpack/bin/fleet-daemon" ] || die "archive does not contain bin/fleet-daemon"

if [ -n "$app_dest" ]; then
  if [ -d "$tmp/unpack/Fleet.app" ]; then
    mkdir -p "$(dirname "$app_dest")"
    rm -rf "$app_dest"
    if command -v ditto >/dev/null 2>&1; then
      ditto "$tmp/unpack/Fleet.app" "$app_dest"
    else
      cp -R "$tmp/unpack/Fleet.app" "$app_dest"
    fi
    lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
    if [ -x "$lsregister" ]; then
      "$lsregister" -f "$app_dest" >/dev/null 2>&1 || true
    fi
    log "Installed Fleet.app to $app_dest"
  else
    warn "archive has no Fleet.app; launchd items will show the signing identity"
  fi
fi

set -- install --from-dir "$tmp/unpack" --prefix "$PREFIX"
if [ -n "$PORT" ]; then
  set -- "$@" --port "$PORT"
fi
if [ "$YES" -eq 1 ]; then
  set -- "$@" --yes
fi
if [ "$NO_SERVICE" -eq 1 ]; then
  set -- "$@" --no-service
fi
if [ "$FORCE" -eq 1 ]; then
  set -- "$@" --force
fi

"$tmp/unpack/bin/fleet" "$@"

PATH="$PREFIX/bin:$PATH"
export PATH
version="$("$PREFIX/bin/fleet" --version 2>/dev/null || true)"
if [ -n "$version" ]; then
  log "Installed $version"
fi
