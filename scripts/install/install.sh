#!/usr/bin/env bash
# Install fleet binaries, web assets and (optionally) systemd units.
#
# This script does not fetch release artifacts. Build first:
#   pnpm build:release
#   pnpm build:web
#
# Usage:
#   sudo ./scripts/install/install.sh [--prefix /usr/local] [--user fleet]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PREFIX="/usr/local"
SERVICE_USER="fleet"
INSTALL_SYSTEMD=1

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="${2:?}"; shift 2 ;;
    --user) SERVICE_USER="${2:?}"; shift 2 ;;
    --no-systemd) INSTALL_SYSTEMD=0; shift ;;
    -h|--help)
      sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then
  echo "run as root (or with sudo) to install into $PREFIX" >&2
  exit 1
fi

BIN_DIR="$PREFIX/bin"
WEB_DIR="$PREFIX/share/fleet/web"
ETC_DIR="/etc/fleet"
STATE_DIR="/var/lib/fleet"

for bin in agent-admin fleet-daemon af; do
  src="$ROOT/target/release/$bin"
  [ -x "$src" ] || { echo "missing $src; run 'pnpm build:release' first" >&2; exit 1; }
done
[ -f "$ROOT/apps/web/dist/index.html" ] || { echo "missing apps/web/dist; run 'pnpm build:web' first" >&2; exit 1; }

if ! id "$SERVICE_USER" >/dev/null 2>&1; then
  useradd --system --create-home --home-dir "$STATE_DIR" --shell /usr/sbin/nologin "$SERVICE_USER"
fi

install -d -m 0755 "$BIN_DIR" "$WEB_DIR" "$ETC_DIR"
install -m 0755 "$ROOT/target/release/agent-admin" "$BIN_DIR/agent-admin"
install -m 0755 "$ROOT/target/release/fleet-daemon" "$BIN_DIR/fleet-daemon"
install -m 0755 "$ROOT/target/release/af" "$BIN_DIR/af"
cp -a "$ROOT/apps/web/dist/." "$WEB_DIR/"
install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" "$STATE_DIR"

# Do not overwrite existing configuration or credentials.
if [ ! -f "$ETC_DIR/admin.toml" ]; then
  cat >"$ETC_DIR/admin.toml" <<TOML
[server]
host = "127.0.0.1"
port = 8787

[storage]
database = "$STATE_DIR/agent-admin.db"

[auth]
# Set AGENT_FLEET_ADMIN_TOKEN and AGENT_FLEET_MACHINE_TOKEN in admin.env.
TOML
  chmod 0640 "$ETC_DIR/admin.toml"
fi
if [ ! -f "$ETC_DIR/daemon.toml" ]; then
  cat >"$ETC_DIR/daemon.toml" <<TOML
[machine]
name = "$(hostname)"
key_file = "$STATE_DIR/machine.key"

[storage]
database = "$STATE_DIR/agent.db"

[agent]
default_provider = "mock"

[connection]
server_url = "ws://127.0.0.1:8787/ws/machine"
TOML
  chmod 0640 "$ETC_DIR/daemon.toml"
fi
if [ ! -f "$ETC_DIR/admin.env" ]; then
  cat >"$ETC_DIR/admin.env" <<'ENV'
# AGENT_FLEET_ADMIN_TOKEN=replace-with-a-long-random-token
# AGENT_FLEET_MACHINE_TOKEN=replace-with-a-different-long-random-token
ENV
  chmod 0600 "$ETC_DIR/admin.env"
fi
if [ ! -f "$ETC_DIR/daemon.env" ]; then
  cat >"$ETC_DIR/daemon.env" <<'ENV'
# AGENT_FLEET_TOKEN=replace-with-the-machine-token
ENV
  chmod 0600 "$ETC_DIR/daemon.env"
fi
chown -R "$SERVICE_USER:$SERVICE_USER" "$STATE_DIR" "$ETC_DIR/admin.toml" "$ETC_DIR/daemon.toml"

if [ "$INSTALL_SYSTEMD" -eq 1 ] && command -v systemctl >/dev/null 2>&1; then
  install -m 0644 "$ROOT/packaging/systemd/agent-admin.service" /etc/systemd/system/agent-admin.service
  install -m 0644 "$ROOT/packaging/systemd/agent-daemon.service" /etc/systemd/system/agent-daemon.service
  systemctl daemon-reload
  echo "systemd units installed; edit /etc/fleet/*.env then:"
  echo "  systemctl enable --now agent-admin agent-daemon"
else
  echo "binaries and web assets installed under $PREFIX"
fi

cat <<EOF

Installed:
  binaries  $BIN_DIR/agent-admin, $BIN_DIR/fleet-daemon, $BIN_DIR/af
  web       $WEB_DIR
  config    $ETC_DIR
  state     $STATE_DIR

Before starting:
  1. Put strong tokens in $ETC_DIR/admin.env and $ETC_DIR/daemon.env.
  2. Review the daemon's access to repositories and agent CLIs.
  3. Keep the admin on loopback; use Tailscale/Cloudflare/reverse proxy for remote access.
EOF
