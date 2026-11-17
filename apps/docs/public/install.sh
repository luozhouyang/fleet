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
#   --service           Install and start the native service: systemd on Linux
#                       (root only) or launchd on macOS (user or system scope)
#   --no-app            macOS: do not install the Fleet.app wrapper
#   --app-dir DIR       macOS: where to install Fleet.app (default:
#                       /Applications, or <prefix>/share/fleet when
#                       /Applications is not writable)
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
INSTALL_APP=1
APP_DIR=""

usage() {
  cat <<'EOF'
Fleet installer for Linux, macOS, and WSL2.

Usage:
  curl -fsSL https://luozhouyang.github.io/fleet/install.sh | sh
  sh install.sh [options]

Options:
  --version VERSION   Release to install (default: latest, e.g. v0.1.0)
  --prefix DIR        Install prefix (default: ~/.local, or /usr/local as root)
  --service           Install and start systemd (Linux, root) or launchd (macOS)
  --no-app            macOS: skip the Fleet.app wrapper
  --app-dir DIR       macOS: Fleet.app install directory (default:
                      /Applications, falling back to <prefix>/share/fleet)
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
    --no-app)
      INSTALL_APP=0
      shift
      ;;
    --app-dir)
      [ $# -ge 2 ] || die "--app-dir requires a value"
      APP_DIR="$2"
      shift 2
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
  app_base="$APP_DIR"
  if [ -z "$app_base" ]; then
    if [ -e "/Applications/Fleet.app" ]; then
      if [ ! -w "/Applications/Fleet.app" ] || [ ! -w /Applications ]; then
        die "cannot replace /Applications/Fleet.app; rerun with sudo or pass --app-dir"
      fi
      app_base="/Applications"
    elif [ -w /Applications ]; then
      app_base="/Applications"
    else
      app_base="$PREFIX/share/fleet"
      warn "/Applications is not writable; installing Fleet.app to $app_base"
    fi
  elif [ -e "$app_base/Fleet.app" ] && [ ! -w "$app_base/Fleet.app" ]; then
    die "$app_base/Fleet.app exists but is not writable; rerun with sudo or choose another --app-dir"
  fi
  app_dest="$app_base/Fleet.app"
fi

log "Fleet installer"
log "  platform: $os/$arch"
log "  release:  $VERSION"
log "  prefix:   $PREFIX"
if [ -n "$app_dest" ]; then
  log "  app:      $app_dest"
fi
log "  source:   $base/$asset"

if [ "$DRY_RUN" -eq 1 ]; then
  log "dry run: nothing was downloaded or written"
  exit 0
fi

if [ "$SERVICE" -eq 1 ] && [ "$os" = "linux" ]; then
  [ "$(id -u)" -eq 0 ] || die "--service requires root on Linux; rerun with sudo"
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

if [ -n "$app_dest" ]; then
  if [ -d "$tmp/unpack/Fleet.app" ]; then
    mkdir -p "$(dirname "$app_dest")"
    rm -rf "$app_dest"
    if command -v ditto >/dev/null 2>&1; then
      ditto "$tmp/unpack/Fleet.app" "$app_dest"
    else
      cp -R "$tmp/unpack/Fleet.app" "$app_dest"
    fi
    if [ "$(id -u)" -eq 0 ]; then
      chown -R root:wheel "$app_dest" 2>/dev/null || true
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

PATH="$PREFIX/bin:$PATH"
export PATH
version="$("$PREFIX/bin/fleet" --version 2>/dev/null || true)"
if [ -n "$version" ]; then
  log "Installed $version"
else
  log "Installed fleet-admin, fleet-daemon, and fleet"
fi

admin_token_from_config() {
  local file="$1"
  [ -f "$file" ] || return 0
  sed -n 's/^[[:space:]]*admin_token[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$file" | tail -n 1
}

wait_for_admin() {
  local i=0
  while [ "$i" -lt 30 ]; do
    if curl -fsS http://127.0.0.1:8787/api/v1/health >/dev/null 2>&1; then
      return 0
    fi
    i=$((i + 1))
    sleep 1
  done
  return 1
}

if [ "$SERVICE" -eq 1 ]; then
  admin_web="$PREFIX/share/fleet/web"
  admin_token=""
  case "$os" in
    linux)
      log "Creating and starting the admin service"
      "$PREFIX/bin/fleet" admin setup --system --yes --start --web-dir "$admin_web"
      if [ -f /etc/fleet/admin.env ]; then
        admin_token="$(sed -n 's/^FLEET_ADMIN_TOKEN=//p' /etc/fleet/admin.env | tail -n 1 | tr -d "'\"")"
      fi
      scope_flag="--system"
      ;;
    darwin)
      if [ "$(id -u)" -eq 0 ]; then
        scope_flag="--system"
        admin_config="/etc/fleet/admin.toml"
      else
        scope_flag="--user"
        admin_config="$HOME/.fleet/admin/config.toml"
      fi
      log "Creating and starting the admin service with launchd"
      "$PREFIX/bin/fleet" admin setup "$scope_flag" --yes --start --install-service \
        --web-dir "$admin_web"
      admin_token="$(admin_token_from_config "$admin_config")"
      ;;
  esac

  if wait_for_admin; then
    log "Enrolling the local daemon"
    if [ -n "$admin_token" ]; then
      FLEET_ADMIN_TOKEN="$admin_token" "$PREFIX/bin/fleet" daemon setup "$scope_flag" --yes --start \
        --install-service \
        --org org_default \
        --server ws://127.0.0.1:8787/ws/machine
    else
      "$PREFIX/bin/fleet" daemon setup "$scope_flag" --yes --start \
        --install-service \
        --org org_default \
        --server ws://127.0.0.1:8787/ws/machine
    fi
    log "Admin: http://127.0.0.1:8787"
    log "Show the admin token: fleet admin config show --reveal"
  else
    warn "admin did not become healthy; enroll the daemon later with:"
    case "$os" in
      linux)
        warn "  sudo FLEET_ADMIN_TOKEN=<token> $PREFIX/bin/fleet daemon setup --system --org org_default --server ws://127.0.0.1:8787/ws/machine"
        ;;
      darwin)
        warn "  FLEET_ADMIN_TOKEN=<token> $PREFIX/bin/fleet daemon setup $scope_flag --install-service --org org_default --server ws://127.0.0.1:8787/ws/machine"
        ;;
    esac
  fi
else
  app_line=""
  if [ -n "$app_dest" ]; then
    app_line="
App bundle: $app_dest"
  fi
  cat <<EOF

Fleet is installed in $PREFIX.$app_line

Next steps:
  1. Add it to your PATH if needed:
       export PATH="$PREFIX/bin:\$PATH"
  2. Configure the admin and install its service:
       fleet admin setup --user --install-service --web-dir "$PREFIX/share/fleet/web"
       sudo fleet admin setup --system --install-service --web-dir "$PREFIX/share/fleet/web"
  3. Enroll this machine and install the daemon service:
       fleet daemon setup --user --install-service --org org_default \\
         --server ws://127.0.0.1:8787/ws/machine
  4. Open http://127.0.0.1:8787.

On Linux, \`sudo sh install.sh --prefix /usr/local --service\` creates and
starts systemd units. On macOS, \`--service\` creates and starts user LaunchAgents
when run as a user, or system LaunchDaemons with sudo. The installer also
places Fleet.app in /Applications (falling back to <prefix>/share/fleet when
/Applications is not writable) so macOS shows "Fleet" rather than the
Developer ID name in Login Items & Extensions.
EOF
fi
