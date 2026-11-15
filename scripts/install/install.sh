#!/usr/bin/env bash
# Install fleet binaries, web assets and (optionally) systemd units.
#
# This script does not fetch release artifacts. Build first:
#   pnpm build:release
#   pnpm build:web
#
# Usage:
#   sudo ./scripts/install/install.sh [options]
#
# Options:
#   --prefix DIR        Installation prefix (default: /usr/local)
#   --admin-user USER   User for the admin service (default: fleet)
#   --daemon-user USER  User for the daemon service (default: $SUDO_USER,
#                       or fleet when not run through sudo)
#   --user USER         Set both service users (legacy shorthand)
#   --no-systemd        Do not install or manage systemd units
#   --root DIR          Stage everything under DIR (implies no root needed
#                       and no systemctl calls); intended for tests/chroots
#   --dry-run           Print rendered systemd units and exit
#   -h, --help          Show this help
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

PREFIX="/usr/local"
ETC_DIR="/etc/fleet"
ADMIN_STATE_DIR="/var/lib/fleet-admin"
DAEMON_STATE_DIR="/var/lib/fleet-daemon"
LEGACY_STATE_DIR="/var/lib/fleet"
BACKUP_DIR="/var/lib/fleet-backup"
SYSTEMD_DIR="/etc/systemd/system"

INSTALL_SYSTEMD=1
DRY_RUN=0
ROOT_DIR=""
LEGACY_USER=""
ADMIN_USER=""
DAEMON_USER=""
ADMIN_USER_SET=0
DAEMON_USER_SET=0

usage() {
  sed -n '2,21p' "$0"
}

die() {
  echo "error: $*" >&2
  exit 1
}

warn() {
  echo "warning: $*" >&2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="${2:?--prefix requires a value}"; shift 2 ;;
    --admin-user) ADMIN_USER="${2:?--admin-user requires a value}"; ADMIN_USER_SET=1; shift 2 ;;
    --daemon-user) DAEMON_USER="${2:?--daemon-user requires a value}"; DAEMON_USER_SET=1; shift 2 ;;
    --user) LEGACY_USER="${2:?--user requires a value}"; shift 2 ;;
    --no-systemd) INSTALL_SYSTEMD=0; shift ;;
    --root) ROOT_DIR="${2:?--root requires a value}"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$PREFIX" in
  /*) ;;
  *) die "--prefix must be an absolute path: $PREFIX" ;;
esac

if [ -n "$ROOT_DIR" ]; then
  case "$ROOT_DIR" in
    /*) ;;
    *) die "--root must be an absolute path: $ROOT_DIR" ;;
  esac
  if [ "$DRY_RUN" -eq 0 ]; then
    mkdir -p "$ROOT_DIR"
    ROOT_DIR="$(cd "$ROOT_DIR" && pwd)"
  else
    ROOT_DIR="${ROOT_DIR%/}"
    [ -n "$ROOT_DIR" ] || ROOT_DIR="/"
  fi
  PREFIX="$ROOT_DIR$PREFIX"
  ETC_DIR="$ROOT_DIR$ETC_DIR"
  ADMIN_STATE_DIR="$ROOT_DIR$ADMIN_STATE_DIR"
  DAEMON_STATE_DIR="$ROOT_DIR$DAEMON_STATE_DIR"
  LEGACY_STATE_DIR="$ROOT_DIR$LEGACY_STATE_DIR"
  BACKUP_DIR="$ROOT_DIR$BACKUP_DIR"
  SYSTEMD_DIR="$ROOT_DIR$SYSTEMD_DIR"
fi

BIN_DIR="$PREFIX/bin"
WEB_DIR="$PREFIX/share/fleet/web"

if [ "$ADMIN_USER_SET" -eq 0 ]; then
  ADMIN_USER="${LEGACY_USER:-fleet}"
fi
if [ "$DAEMON_USER_SET" -eq 0 ]; then
  if [ -n "$LEGACY_USER" ]; then
    DAEMON_USER="$LEGACY_USER"
  elif [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    DAEMON_USER="$SUDO_USER"
  else
    DAEMON_USER="fleet"
  fi
fi

validate_user() {
  case "$1" in
    ""|*[!A-Za-z0-9_.-]*) die "invalid user name: $1" ;;
    [A-Za-z0-9_]*) ;;
    *) die "invalid user name: $1" ;;
  esac
}
validate_user "$ADMIN_USER"
validate_user "$DAEMON_USER"

# Root is only required for a real, non-staged installation.
if [ "$DRY_RUN" -eq 0 ] && [ -z "$ROOT_DIR" ] && [ "$(id -u)" -ne 0 ]; then
  die "run as root (or with sudo) to install into $PREFIX"
fi

# In a staged root we never touch host users or ownership.
CAN_CHOWN=0
if [ "$DRY_RUN" -eq 0 ] && [ -z "$ROOT_DIR" ] && [ "$(id -u)" -eq 0 ]; then
  CAN_CHOWN=1
fi

user_group() {
  if id "$1" >/dev/null 2>&1; then
    id -gn "$1"
  else
    printf '%s' "$1"
  fi
}

user_home() {
  local user="$1" fallback="$2" home=""
  if command -v getent >/dev/null 2>&1; then
    home="$(getent passwd "$user" | cut -d: -f6)"
  fi
  printf '%s' "${home:-$fallback}"
}

ensure_user() {
  local user="$1" home="$2"
  if id "$user" >/dev/null 2>&1; then
    return 0
  fi
  if [ "$CAN_CHOWN" -ne 1 ]; then
    warn "user '$user' does not exist; skipping user creation in staged/dry-run mode"
    return 0
  fi
  if ! useradd --system --user-group --create-home --home-dir "$home" \
      --shell /usr/sbin/nologin "$user"; then
    die "failed to create service user '$user'; create it manually and rerun"
  fi
}

if [ "$DRY_RUN" -eq 0 ]; then
  ensure_user "$ADMIN_USER" "$ADMIN_STATE_DIR"
  ensure_user "$DAEMON_USER" "$DAEMON_STATE_DIR"
fi

ADMIN_GROUP="$(user_group "$ADMIN_USER")"
DAEMON_GROUP="$(user_group "$DAEMON_USER")"
ADMIN_HOME="$ADMIN_STATE_DIR"
DAEMON_HOME="$(user_home "$DAEMON_USER" "$DAEMON_STATE_DIR")"

render_unit() {
  local src="$1" content
  content="$(<"$src")"
  content="${content//@ADMIN_USER@/$ADMIN_USER}"
  content="${content//@ADMIN_GROUP@/$ADMIN_GROUP}"
  content="${content//@DAEMON_USER@/$DAEMON_USER}"
  content="${content//@DAEMON_GROUP@/$DAEMON_GROUP}"
  content="${content//@ADMIN_STATE_DIR@/$ADMIN_STATE_DIR}"
  content="${content//@DAEMON_STATE_DIR@/$DAEMON_STATE_DIR}"
  content="${content//@LEGACY_STATE_DIR@/$LEGACY_STATE_DIR}"
  content="${content//@ETC_DIR@/$ETC_DIR}"
  content="${content//@BIN_DIR@/$BIN_DIR}"
  content="${content//@WEB_DIR@/$WEB_DIR}"
  content="${content//@ADMIN_HOME@/$ADMIN_HOME}"
  content="${content//@DAEMON_HOME@/$DAEMON_HOME}"
  printf '%s\n' "$content"
}

ADMIN_UNIT_SRC="$ROOT/packaging/systemd/agent-admin.service.in"
DAEMON_UNIT_SRC="$ROOT/packaging/systemd/agent-daemon.service.in"
[ -f "$ADMIN_UNIT_SRC" ] || die "missing unit template: $ADMIN_UNIT_SRC"
[ -f "$DAEMON_UNIT_SRC" ] || die "missing unit template: $DAEMON_UNIT_SRC"

if [ "$DRY_RUN" -eq 1 ]; then
  printf '=== %s/agent-admin.service ===\n' "$SYSTEMD_DIR"
  render_unit "$ADMIN_UNIT_SRC"
  printf '=== %s/agent-daemon.service ===\n' "$SYSTEMD_DIR"
  render_unit "$DAEMON_UNIT_SRC"
  exit 0
fi

for bin in agent-admin fleet-daemon af; do
  src="$ROOT/target/release/$bin"
  [ -x "$src" ] || die "missing $src; run 'pnpm build:release' first"
done
[ -f "$ROOT/apps/web/dist/index.html" ] || die "missing apps/web/dist; run 'pnpm build:web' first"

make_dir() { # mode owner group path
  local mode="$1" owner="$2" group="$3" path="$4"
  install -d -m "$mode" "$path"
  if [ "$CAN_CHOWN" -eq 1 ]; then
    chown "$owner:$group" "$path"
    chmod "$mode" "$path"
  fi
}

make_dir 0755 root root "$BIN_DIR"
make_dir 0755 root root "$WEB_DIR"
make_dir 0755 root root "$ETC_DIR"
make_dir 0750 "$ADMIN_USER" "$ADMIN_GROUP" "$ADMIN_STATE_DIR"
make_dir 0750 "$DAEMON_USER" "$DAEMON_GROUP" "$DAEMON_STATE_DIR"

# Stop running services before replacing their binaries. Remember which ones
# were active so they can be restarted after the upgrade.
ACTIVE_SERVICES=()
if [ "$INSTALL_SYSTEMD" -eq 1 ] && [ -z "$ROOT_DIR" ] && command -v systemctl >/dev/null 2>&1; then
  for svc in agent-admin agent-daemon; do
    if systemctl is-active --quiet "$svc" 2>/dev/null; then
      ACTIVE_SERVICES+=("$svc")
    fi
  done
  if [ ${#ACTIVE_SERVICES[@]} -gt 0 ]; then
    echo "stopping active services: ${ACTIVE_SERVICES[*]}"
    systemctl stop "${ACTIVE_SERVICES[@]}"
  fi
fi

on_error() {
  local status=$?
  trap - ERR
  if [ ${#ACTIVE_SERVICES[@]} -gt 0 ]; then
    echo "install failed; restarting previously active services" >&2
    systemctl start "${ACTIVE_SERVICES[@]}" || true
  fi
  exit "$status"
}
trap on_error ERR

# Back up the current binaries and units so an upgrade can be rolled back.
BACKUP_DEST=""
backup_existing() {
  local files=() bin stamp dest unit
  for bin in agent-admin fleet-daemon af; do
    [ -e "$BIN_DIR/$bin" ] && files+=("$BIN_DIR/$bin")
  done
  [ ${#files[@]} -gt 0 ] || return 0

  stamp="$(date -u +%Y%m%dT%H%M%SZ)-$$"
  dest="$BACKUP_DIR/$stamp"
  install -d -m 0750 "$dest"
  cp -a "${files[@]}" "$dest/"
  for unit in agent-admin.service agent-daemon.service; do
    if [ -f "$SYSTEMD_DIR/$unit" ]; then
      cp -a "$SYSTEMD_DIR/$unit" "$dest/$unit"
    fi
  done
  BACKUP_DEST="$dest"
}
backup_existing

install -m 0755 "$ROOT/target/release/agent-admin" "$BIN_DIR/agent-admin"
install -m 0755 "$ROOT/target/release/fleet-daemon" "$BIN_DIR/fleet-daemon"
install -m 0755 "$ROOT/target/release/af" "$BIN_DIR/af"

# Replace web assets so stale hashed bundles do not accumulate.
if [ -L "$WEB_DIR" ]; then
  rm -f "$WEB_DIR"
  install -d -m 0755 "$WEB_DIR"
elif [ -d "$WEB_DIR" ]; then
  rm -rf "$WEB_DIR"
fi
install -d -m 0755 "$WEB_DIR"
cp -a "$ROOT/apps/web/dist/." "$WEB_DIR/"

# Configuration is only created when missing so upgrades keep existing paths.
if [ ! -f "$ETC_DIR/admin.toml" ]; then
  cat >"$ETC_DIR/admin.toml" <<TOML
[server]
host = "127.0.0.1"
port = 8787

[storage]
database = "$ADMIN_STATE_DIR/agent-admin.db"

[auth]
# Tokens are read from admin.env (AGENT_FLEET_ADMIN_TOKEN and
# AGENT_FLEET_MACHINE_TOKEN).
TOML
fi
if [ ! -f "$ETC_DIR/daemon.toml" ]; then
  cat >"$ETC_DIR/daemon.toml" <<TOML
[machine]
name = "$(hostname)"
key_file = "$DAEMON_STATE_DIR/machine.key"

[storage]
database = "$DAEMON_STATE_DIR/agent.db"

[agent]
default_provider = "mock"

[connection]
server_url = "ws://127.0.0.1:8787/ws/machine"
TOML
fi

ensure_env_file() {
  local file="$1"
  if [ ! -f "$file" ]; then
    cat >"$file" <<'ENV'
# fleet service environment. Tokens are generated on first install when absent.
ENV
  fi
}

append_line() {
  local file="$1" line="$2"
  if [ -s "$file" ] && [ "$(tail -c 1 "$file" | wc -l)" -eq 0 ]; then
    printf '\n' >>"$file"
  fi
  printf '%s\n' "$line" >>"$file"
}

env_get() { # file key -> value (last uncommented assignment)
  local file="$1" key="$2" line value
  [ -f "$file" ] || return 1
  line="$(grep -E "^[[:space:]]*${key}[[:space:]]*=" "$file" | tail -n 1 || true)"
  [ -n "$line" ] || return 1
  value="${line#*=}"
  value="$(printf '%s' "$value" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
  value="${value%\"}"; value="${value#\"}"
  value="${value%\'}"; value="${value#\'}"
  [ -n "$value" ] || return 1
  printf '%s' "$value"
}

generate_token() {
  local prefix="$1"
  if command -v openssl >/dev/null 2>&1; then
    printf '%s_%s\n' "$prefix" "$(openssl rand -hex 32)"
  elif [ -r /dev/urandom ]; then
    printf '%s_%s\n' "$prefix" "$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  else
    return 1
  fi
}

ensure_env_file "$ETC_DIR/admin.env"
ensure_env_file "$ETC_DIR/daemon.env"

ADMIN_TOKEN_VAL="$(env_get "$ETC_DIR/admin.env" AGENT_FLEET_ADMIN_TOKEN || true)"
if [ -z "$ADMIN_TOKEN_VAL" ]; then
  ADMIN_TOKEN_VAL="$(generate_token af_admin)" \
    || die "cannot generate an admin token; install openssl or write AGENT_FLEET_ADMIN_TOKEN to $ETC_DIR/admin.env"
  append_line "$ETC_DIR/admin.env" "AGENT_FLEET_ADMIN_TOKEN=$ADMIN_TOKEN_VAL"
fi

MACHINE_TOKEN_VAL="$(env_get "$ETC_DIR/admin.env" AGENT_FLEET_MACHINE_TOKEN || true)"
DAEMON_TOKEN_VAL="$(env_get "$ETC_DIR/daemon.env" AGENT_FLEET_TOKEN || true)"
if [ -z "$MACHINE_TOKEN_VAL" ] && [ -n "$DAEMON_TOKEN_VAL" ]; then
  append_line "$ETC_DIR/admin.env" "AGENT_FLEET_MACHINE_TOKEN=$DAEMON_TOKEN_VAL"
elif [ -z "$MACHINE_TOKEN_VAL" ] && [ -z "$DAEMON_TOKEN_VAL" ]; then
  MACHINE_TOKEN_VAL="$(generate_token af_machine)" \
    || die "cannot generate a machine token; install openssl or set AGENT_FLEET_MACHINE_TOKEN and AGENT_FLEET_TOKEN in $ETC_DIR"
  append_line "$ETC_DIR/admin.env" "AGENT_FLEET_MACHINE_TOKEN=$MACHINE_TOKEN_VAL"
  append_line "$ETC_DIR/daemon.env" "AGENT_FLEET_TOKEN=$MACHINE_TOKEN_VAL"
elif [ -n "$MACHINE_TOKEN_VAL" ] && [ -z "$DAEMON_TOKEN_VAL" ]; then
  append_line "$ETC_DIR/daemon.env" "AGENT_FLEET_TOKEN=$MACHINE_TOKEN_VAL"
elif [ "$MACHINE_TOKEN_VAL" != "$DAEMON_TOKEN_VAL" ]; then
  warn "AGENT_FLEET_MACHINE_TOKEN (admin.env) and AGENT_FLEET_TOKEN (daemon.env) differ; the daemon will not register until they match"
fi

set_file_access() { # path owner group mode
  local path="$1" owner="$2" group="$3" mode="$4"
  chmod "$mode" "$path"
  if [ "$CAN_CHOWN" -eq 1 ]; then
    chown "$owner:$group" "$path"
  fi
}

set_file_access "$ETC_DIR/admin.toml" "$ADMIN_USER" "$ADMIN_GROUP" 0640
set_file_access "$ETC_DIR/daemon.toml" "$DAEMON_USER" "$DAEMON_GROUP" 0640
set_file_access "$ETC_DIR/admin.env" "$ADMIN_USER" "$ADMIN_GROUP" 0600
set_file_access "$ETC_DIR/daemon.env" "$DAEMON_USER" "$DAEMON_GROUP" 0600

if [ "$CAN_CHOWN" -eq 1 ]; then
  # Existing state directories may predate a user change.
  chown -R "$ADMIN_USER:$ADMIN_GROUP" "$ADMIN_STATE_DIR"
  chown -R "$DAEMON_USER:$DAEMON_GROUP" "$DAEMON_STATE_DIR"
fi

# Pre-split installs stored both databases in /var/lib/fleet and kept existing
# config paths. Keep that directory usable when both service users differ.
prepare_legacy_state() {
  [ -d "$LEGACY_STATE_DIR" ] || return 0
  [ "$LEGACY_STATE_DIR" != "$ADMIN_STATE_DIR" ] || return 0
  [ "$LEGACY_STATE_DIR" != "$DAEMON_STATE_DIR" ] || return 0
  if [ "$CAN_CHOWN" -ne 1 ]; then
    warn "legacy state exists at $LEGACY_STATE_DIR; ensure both service users can access it"
    return 0
  fi

  if [ "$ADMIN_GROUP" != "$DAEMON_GROUP" ] && id "$DAEMON_USER" >/dev/null 2>&1; then
    usermod -aG "$ADMIN_GROUP" "$DAEMON_USER" \
      || warn "could not add $DAEMON_USER to group $ADMIN_GROUP"
  fi
  chgrp -R "$ADMIN_GROUP" "$LEGACY_STATE_DIR" 2>/dev/null || true
  chmod 2770 "$LEGACY_STATE_DIR" || true

  local f
  shopt -s nullglob
  for f in "$LEGACY_STATE_DIR"/agent-admin.db*; do
    chown "$ADMIN_USER:$ADMIN_GROUP" "$f" || true
  done
  for f in "$LEGACY_STATE_DIR"/agent.db* "$LEGACY_STATE_DIR"/machine.key* \
           "$LEGACY_STATE_DIR"/machine.id; do
    [ -e "$f" ] || continue
    chown "$DAEMON_USER:$DAEMON_GROUP" "$f" || true
    case "$f" in
      */machine.key*) chmod 0600 "$f" || true ;;
    esac
  done
  shopt -u nullglob
  echo "note: existing configs still use legacy state in $LEGACY_STATE_DIR"
}
prepare_legacy_state

# Install rendered units. They are written to a staged root without touching
# the host when --root is used.
install_units() {
  install -d -m 0755 "$SYSTEMD_DIR"
  render_unit "$ADMIN_UNIT_SRC" >"$SYSTEMD_DIR/agent-admin.service"
  render_unit "$DAEMON_UNIT_SRC" >"$SYSTEMD_DIR/agent-daemon.service"
  chmod 0644 "$SYSTEMD_DIR/agent-admin.service" "$SYSTEMD_DIR/agent-daemon.service"
}

if [ "$INSTALL_SYSTEMD" -eq 1 ]; then
  install_units
  if [ -n "$ROOT_DIR" ]; then
    echo "systemd units staged in $SYSTEMD_DIR (no systemctl calls for --root)"
  elif command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
    if [ ${#ACTIVE_SERVICES[@]} -gt 0 ]; then
      for svc in "${ACTIVE_SERVICES[@]}"; do
        systemctl restart "$svc" || warn "failed to restart $svc"
      done
    fi
    if [ ${#ACTIVE_SERVICES[@]} -eq 0 ]; then
      echo "systemd units installed; start them with:"
      echo "  systemctl enable --now agent-admin agent-daemon"
    else
      echo "restarted previously active services: ${ACTIVE_SERVICES[*]}"
    fi
  else
    echo "systemctl not found; units written to $SYSTEMD_DIR but not activated"
  fi
else
  echo "skipping systemd unit installation (--no-systemd)"
fi

trap - ERR

if [ -n "$BACKUP_DEST" ]; then
  echo "previous binaries and units backed up to $BACKUP_DEST"
  echo "rollback: stop services, copy $BACKUP_DEST/*.service back to $SYSTEMD_DIR,"
  echo "          copy the binaries back to $BIN_DIR, then run systemctl daemon-reload"
fi

cat <<EOF

Installed:
  binaries  $BIN_DIR/agent-admin, $BIN_DIR/fleet-daemon, $BIN_DIR/af
  web       $WEB_DIR
  config    $ETC_DIR (admin: $ADMIN_USER, daemon: $DAEMON_USER)
  state     admin $ADMIN_STATE_DIR, daemon $DAEMON_STATE_DIR
  backups   $BACKUP_DIR

Before starting:
  1. Review the generated tokens in $ETC_DIR/admin.env and $ETC_DIR/daemon.env.
  2. The daemon runs as $DAEMON_USER and needs access to repositories and the
     agent CLIs (claude/codex/opencode) with their credentials.
  3. The admin binds loopback; use Tailscale/Cloudflare/reverse proxy for remote access.
EOF
