#!/bin/sh
# Fleet installer for Linux, macOS, and WSL2.
#
# One-line install:
#   curl -fsSL https://luozhouyang.github.io/fleet/install.sh | sh
#
# Inspect first (recommended):
#   curl -fsSLO https://luozhouyang.github.io/fleet/install.sh
#   less install.sh && sh install.sh
#
# Options:
#   --version VERSION   Release to install (default: latest, e.g. v0.1.0)
#   --prefix DIR        Install prefix (default: ~/.local, or /usr/local as root)
#   --service           Linux root only: create and start systemd services
#   --dry-run           Print the plan and exit without writing anything
#   --force             Reinstall over an existing prefix
#   -h, --help          Show this help
#
# Environment:
#   FLEET_VERSION       Same as --version
#   FLEET_PREFIX        Same as --prefix
#   FLEET_BASE_URL      Override the release download base (for mirrors/tests)
#
# The installer downloads a platform archive from:
#   https://github.com/luozhouyang/fleet/releases
# and verifies it against SHA256SUMS when the release publishes one.

set -eu

REPO="luozhouyang/fleet"
VERSION="${FLEET_VERSION:-latest}"
PREFIX="${FLEET_PREFIX:-}"
SERVICE=0
DRY_RUN=0
FORCE=0

usage() {
  cat <<'EOF'
Fleet installer for Linux, macOS, and WSL2.

Usage:
  curl -fsSL https://luozhouyang.github.io/fleet/install.sh | sh
  sh install.sh [options]

Options:
  --version VERSION   Release to install (default: latest, e.g. v0.1.0)
  --prefix DIR        Install prefix (default: ~/.local, or /usr/local as root)
  --service           Linux root only: create and start systemd services
  --dry-run           Print the plan and exit without writing anything
  --force             Reinstall over an existing prefix
  -h, --help          Show this help

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
    --service)
      SERVICE=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --force)
      FORCE=1
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
  *)
    die "unsupported operating system: $os (on Windows, use WSL2)"
    ;;
esac

machine="$(uname -m)"
case "$machine" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="arm64" ;;
  *) die "unsupported architecture: $machine" ;;
esac

asset="fleet-$os-$arch.tar.gz"

if [ -z "$PREFIX" ]; then
  if [ "$(id -u)" -eq 0 ]; then
    PREFIX="/usr/local"
  else
    PREFIX="$HOME/.local"
  fi
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

if [ "$SERVICE" -eq 1 ]; then
  [ "$os" = "linux" ] || die "--service is supported on Linux only"
  [ "$(id -u)" -eq 0 ] || die "--service requires root; rerun with sudo"
fi

log "Fleet installer"
log "  platform: $os/$arch"
log "  release:  $VERSION"
log "  prefix:   $PREFIX"
log "  source:   $base/$asset"

if [ "$DRY_RUN" -eq 1 ]; then
  log "dry run: nothing was downloaded or written"
  exit 0
fi

if [ "$SERVICE" -eq 1 ]; then
  command -v systemctl >/dev/null 2>&1 || die "--service requires systemd"
  systemctl stop fleet-admin fleet-daemon 2>/dev/null || true
fi

if [ -e "$PREFIX/bin/fleet" ] && [ "$FORCE" -ne 1 ]; then
  die "$PREFIX/bin/fleet already exists; pass --force to reinstall"
fi

if [ ! -w "$PREFIX" ] && [ "$(id -u)" -ne 0 ]; then
  parent="$(dirname "$PREFIX")"
  if [ ! -d "$parent" ] || [ ! -w "$parent" ]; then
    die "cannot write to $PREFIX; rerun with sudo or choose --prefix \"$HOME/.local\""
  fi
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

log "Installing binaries to $PREFIX/bin"
mkdir -p "$PREFIX/bin"
install -m 0755 "$tmp/unpack/bin/fleet" "$PREFIX/bin/fleet"
install -m 0755 "$tmp/unpack/bin/fleet-admin" "$PREFIX/bin/fleet-admin"
install -m 0755 "$tmp/unpack/bin/fleet-daemon" "$PREFIX/bin/fleet-daemon"

if [ -d "$tmp/unpack/web" ]; then
  web_dir="$PREFIX/share/fleet/web"
  log "Installing web UI to $web_dir"
  mkdir -p "$web_dir"
  rm -rf "$web_dir"
  mkdir -p "$web_dir"
  cp -R "$tmp/unpack/web/." "$web_dir/"
else
  warn "archive has no web/ directory; fleet-admin will need --web-dir"
fi

PATH="$PREFIX/bin:$PATH"
export PATH
version="$("$PREFIX/bin/fleet" --version 2>/dev/null || true)"
if [ -n "$version" ]; then
  log "Installed $version"
else
  log "Installed fleet-admin, fleet-daemon, and fleet"
fi

if [ "$SERVICE" -eq 1 ]; then
  admin_web="$PREFIX/share/fleet/web"
  log "Creating and starting the admin service"
  "$PREFIX/bin/fleet" admin setup --system --yes --start --web-dir "$admin_web"

  admin_token=""
  if [ -f /etc/fleet/admin.env ]; then
    admin_token="$(sed -n 's/^FLEET_ADMIN_TOKEN=//p' /etc/fleet/admin.env | tail -n 1 | tr -d "'\"")"
  fi

  i=0
  while [ "$i" -lt 30 ]; do
    if curl -fsS http://127.0.0.1:8787/api/v1/health >/dev/null 2>&1; then
      break
    fi
    i=$((i + 1))
    sleep 1
  done

  if curl -fsS http://127.0.0.1:8787/api/v1/health >/dev/null 2>&1; then
    log "Enrolling the local daemon"
    if [ -n "$admin_token" ]; then
      FLEET_ADMIN_TOKEN="$admin_token" "$PREFIX/bin/fleet" daemon setup --system --yes --start \
        --org org_default \
        --server ws://127.0.0.1:8787/ws/machine
    else
      "$PREFIX/bin/fleet" daemon setup --system --yes --start \
        --org org_default \
        --server ws://127.0.0.1:8787/ws/machine
    fi
    log "Admin: http://127.0.0.1:8787"
    log "Show the admin token: fleet admin config show --reveal"
  else
    warn "admin did not become healthy; enroll the daemon later with:"
    warn "  sudo FLEET_ADMIN_TOKEN=<token> fleet daemon setup --system --org org_default --server ws://127.0.0.1:8787/ws/machine"
  fi
else
  cat <<EOF

Fleet is installed in $PREFIX.

Next steps:
  1. Add it to your PATH if needed:
       export PATH="$PREFIX/bin:\$PATH"
  2. Configure the admin:
       fleet admin setup --user --web-dir "$PREFIX/share/fleet/web"
       sudo fleet admin setup --system --web-dir "$PREFIX/share/fleet/web"
  3. Enroll this machine:
       fleet daemon setup --org org_default \\
         --server ws://127.0.0.1:8787/ws/machine
  4. Open http://127.0.0.1:8787.

On Linux, \`sudo sh install.sh --prefix /usr/local --service\` creates and
starts systemd units. On macOS, use \`fleet admin setup --user\` and the
fallback supervisor, or install a launchd unit manually.
EOF
fi
