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
#   --admin-user USER   User for the admin service (default: fleet-admin)
#   --daemon-user USER  User for the daemon service (default: $SUDO_USER,
#                       or fleet-daemon when not run through sudo)
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
ADMIN_STATE_DIR="/var/lib/fleet/admin"
DAEMON_STATE_DIR="/var/lib/fleet/daemon"
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
  BACKUP_DIR="$ROOT_DIR$BACKUP_DIR"
  SYSTEMD_DIR="$ROOT_DIR$SYSTEMD_DIR"
fi

BIN_DIR="$PREFIX/bin"
WEB_DIR="$PREFIX/share/fleet/web"

if [ "$ADMIN_USER_SET" -eq 0 ]; then
  ADMIN_USER="${LEGACY_USER:-fleet-admin}"
fi
if [ "$DAEMON_USER_SET" -eq 0 ]; then
  if [ -n "$LEGACY_USER" ]; then
    DAEMON_USER="$LEGACY_USER"
  elif [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    DAEMON_USER="$SUDO_USER"
  else
    DAEMON_USER="fleet-daemon"
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

render_unit() { # src config-file env-file
  local src="$1" config_file="$2" env_file="$3" content
  content="$(<"$src")"
  content="${content//@ADMIN_USER@/$ADMIN_USER}"
  content="${content//@ADMIN_GROUP@/$ADMIN_GROUP}"
  content="${content//@DAEMON_USER@/$DAEMON_USER}"
  content="${content//@DAEMON_GROUP@/$DAEMON_GROUP}"
  content="${content//@ADMIN_STATE_DIR@/$ADMIN_STATE_DIR}"
  content="${content//@DAEMON_STATE_DIR@/$DAEMON_STATE_DIR}"
  content="${content//@ADMIN_HOME@/$ADMIN_HOME}"
  content="${content//@DAEMON_HOME@/$DAEMON_HOME}"
  content="${content//@FLEET_BIN@/$BIN_DIR/fleet}"
  content="${content//@CONFIG_FILE@/$config_file}"
  content="${content//@ENV_FILE@/$env_file}"
  printf '%s\n' "$content"
}

ADMIN_UNIT_SRC="$ROOT/packaging/systemd/fleet-admin.service.in"
DAEMON_UNIT_SRC="$ROOT/packaging/systemd/fleet-daemon.service.in"
[ -f "$ADMIN_UNIT_SRC" ] || die "missing unit template: $ADMIN_UNIT_SRC"
[ -f "$DAEMON_UNIT_SRC" ] || die "missing unit template: $DAEMON_UNIT_SRC"

if [ "$DRY_RUN" -eq 1 ]; then
  printf '=== %s/fleet-admin.service ===\n' "$SYSTEMD_DIR"
  render_unit "$ADMIN_UNIT_SRC" "$ETC_DIR/admin.toml" "$ETC_DIR/admin.env"
  printf '=== %s/fleet-daemon.service ===\n' "$SYSTEMD_DIR"
  render_unit "$DAEMON_UNIT_SRC" "$ETC_DIR/daemon.toml" "$ETC_DIR/daemon.env"
  exit 0
fi

# One-time removal of pre-rebrand binaries and units. This is the only place
# allowed to name the old binaries and units; every line that does carries the
# `fleet-legacy-cleanup` marker and verification scans filter those lines.
if [ "$DRY_RUN" -eq 0 ]; then
  rm -f "$BIN_DIR/af" "$BIN_DIR/agent-admin" # fleet-legacy-cleanup
fi

if [ "$INSTALL_SYSTEMD" -eq 1 ]; then
  if [ -z "$ROOT_DIR" ]; then
    systemctl disable --now agent-admin agent-daemon 2>/dev/null || true # fleet-legacy-cleanup
  fi
  rm -f "$SYSTEMD_DIR/agent-admin.service" "$SYSTEMD_DIR/agent-daemon.service" # fleet-legacy-cleanup
  if [ -z "$ROOT_DIR" ]; then
    systemctl daemon-reload 2>/dev/null || true
  fi
fi

for bin in fleet-admin fleet-daemon fleet; do
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
  for svc in fleet-admin fleet-daemon; do
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
  for bin in fleet-admin fleet-daemon fleet; do
    [ -e "$BIN_DIR/$bin" ] && files+=("$BIN_DIR/$bin")
  done
  [ ${#files[@]} -gt 0 ] || return 0

  stamp="$(date -u +%Y%m%dT%H%M%SZ)-$$"
  dest="$BACKUP_DIR/$stamp"
  install -d -m 0750 "$dest"
  cp -a "${files[@]}" "$dest/"
  for unit in fleet-admin.service fleet-daemon.service; do
    if [ -f "$SYSTEMD_DIR/$unit" ]; then
      cp -a "$SYSTEMD_DIR/$unit" "$dest/$unit"
    fi
  done
  BACKUP_DEST="$dest"
}
backup_existing

install -m 0755 "$ROOT/target/release/fleet-admin" "$BIN_DIR/fleet-admin"
install -m 0755 "$ROOT/target/release/fleet-daemon" "$BIN_DIR/fleet-daemon"
install -m 0755 "$ROOT/target/release/fleet" "$BIN_DIR/fleet"

# Replace web assets so stale hashed bundles do not accumulate.
if [ -L "$WEB_DIR" ]; then
  rm -f "$WEB_DIR"
  install -d -m 0755 "$WEB_DIR"
elif [ -d "$WEB_DIR" ]; then
  rm -rf "$WEB_DIR"
fi
install -d -m 0755 "$WEB_DIR"
cp -a "$ROOT/apps/web/dist/." "$WEB_DIR/"

# Configuration is written on a fresh install. Plan 1 is a clean break with no
# migration from pre-rebrand paths.
if [ ! -f "$ETC_DIR/admin.toml" ]; then
  cat >"$ETC_DIR/admin.toml" <<TOML
[server]
host = "127.0.0.1"
port = 8787

[storage]
database = "$ADMIN_STATE_DIR/fleet-admin.db"

[auth]
# The admin bearer token is read from admin.env (FLEET_ADMIN_TOKEN).
# Daemons enroll with short-lived pairing codes; see auth.pair_code_ttl_secs
# and auth.pair_code_max_attempts.

[web]
static_dir = "$WEB_DIR"
TOML
fi
if [ ! -f "$ETC_DIR/daemon.toml" ]; then
  cat >"$ETC_DIR/daemon.toml" <<TOML
[machine]
name = "$(hostname)"
key_file = "$DAEMON_STATE_DIR/machine.key"

[storage]
database = "$DAEMON_STATE_DIR/fleet-daemon.db"

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
# fleet service environment. The admin token is generated on first install
# when absent; machine credentials live next to the machine key, not here.
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

ADMIN_TOKEN_VAL="$(env_get "$ETC_DIR/admin.env" FLEET_ADMIN_TOKEN || true)"
if [ -z "$ADMIN_TOKEN_VAL" ]; then
  ADMIN_TOKEN_VAL="$(generate_token fla)" \
    || die "cannot generate an admin token; install openssl or write FLEET_ADMIN_TOKEN to $ETC_DIR/admin.env"
  append_line "$ETC_DIR/admin.env" "FLEET_ADMIN_TOKEN=$ADMIN_TOKEN_VAL"
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

# Install rendered units. They are written to a staged root without touching
# the host when --root is used.
install_units() {
  install -d -m 0755 "$SYSTEMD_DIR"
  render_unit "$ADMIN_UNIT_SRC" "$ETC_DIR/admin.toml" "$ETC_DIR/admin.env" \
    >"$SYSTEMD_DIR/fleet-admin.service"
  render_unit "$DAEMON_UNIT_SRC" "$ETC_DIR/daemon.toml" "$ETC_DIR/daemon.env" \
    >"$SYSTEMD_DIR/fleet-daemon.service"
  chmod 0644 "$SYSTEMD_DIR/fleet-admin.service" "$SYSTEMD_DIR/fleet-daemon.service"
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
      echo "  systemctl enable --now fleet-admin fleet-daemon"
    else
      echo "restarted previously active services: ${ACTIVE_SERVICES[*]}"
    fi
  else
    echo "systemctl not found; units written to $SYSTEMD_DIR but not activated"
  fi
else
  echo "skipping systemd unit installation (--no-systemd)"
fi

# Best-effort enrollment of the local daemon. Pairing runs as root so it can
# rewrite /etc/fleet and create the token/key files, then every artifact it may
# have touched is handed to the daemon user. The admin token is never passed to
# a lower-privileged process (argv or environment). A fresh install has not
# started the admin, so the pairing step is skipped with instructions instead of
# blocking; when the admin is active we wait up to ~30 seconds for health.
ADMIN_PORT=""
if [ -f "$ETC_DIR/admin.toml" ]; then
  ADMIN_PORT="$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' \
    "$ETC_DIR/admin.toml" | head -n 1)"
fi
ADMIN_PORT="${ADMIN_PORT:-8787}"

daemon_key_file() {
  local key=""
  if [ -f "$ETC_DIR/daemon.toml" ]; then
    key="$(sed -n 's/^[[:space:]]*key_file[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' \
      "$ETC_DIR/daemon.toml" | head -n 1)"
    if [ -z "$key" ]; then
      key="$(sed -n "s/^[[:space:]]*key_file[[:space:]]*=[[:space:]]*'\([^']*\)'.*/\1/p" \
        "$ETC_DIR/daemon.toml" | head -n 1)"
    fi
  fi
  # Match the core-config default when `key_file` is absent from the TOML.
  key="${key:-$DAEMON_STATE_DIR/machine.key}"
  case "$key" in
    "~") key="$DAEMON_HOME" ;;
    "~/"*) key="$DAEMON_HOME/${key#\~/}" ;;
  esac
  printf '%s' "$key"
}

wait_for_admin() {
  local port="$1" i=0
  if ! command -v curl >/dev/null 2>&1; then
    warn "curl not found; skipping the fleet-admin health wait (pairing stays best-effort)"
    return 0
  fi
  while [ "$i" -lt 60 ]; do
    if curl -fsS "http://127.0.0.1:$port/api/v1/health" >/dev/null 2>&1; then
      return 0
    fi
    i=$((i + 1))
    sleep 0.5
  done
  return 1
}

# Re-own whatever `fleet daemon setup` may have created (key, id, token,
# previous key, config backups) and restore the intended modes. Each step warns
# instead of aborting so a filesystem/group quirk cannot trip the ERR trap.
restore_daemon_ownership() {
  local key file
  key="$(daemon_key_file)"
  for file in "$ETC_DIR/daemon.toml" "$ETC_DIR"/daemon.toml.bak* \
              "$key" "$key.id" "$key.token" "$key.previous"; do
    [ -e "$file" ] || continue
    if [ "$CAN_CHOWN" -eq 1 ]; then
      chown "$DAEMON_USER:$DAEMON_GROUP" "$file" \
        || warn "could not chown $file to $DAEMON_USER:$DAEMON_GROUP"
    fi
    case "$file" in
      "$ETC_DIR/daemon.toml"|"$ETC_DIR"/daemon.toml.bak*)
        chmod 0640 "$file" || warn "could not chmod 0640 $file" ;;
      *)
        chmod 0600 "$file" || warn "could not chmod 0600 $file" ;;
    esac
  done
}

# Runnable hints. The admin token is a literal placeholder and is passed via
# the environment, never argv, so copying the command cannot leak it to `ps`.
pair_command_hint() {
  printf "sudo env HOME=%q FLEET_ADMIN_TOKEN='<admin-token>' %q --admin-url %q daemon setup --system --no-install-service --org org_default --server %q --config %q --name '<machine-name>'" \
    "$DAEMON_HOME" "$BIN_DIR/fleet" "http://127.0.0.1:$ADMIN_PORT" \
    "ws://127.0.0.1:$ADMIN_PORT/ws/machine" "$ETC_DIR/daemon.toml"
}

ownership_restore_hint() {
  local key
  key="$(daemon_key_file)"
  printf 'sudo chown %q:%q %q %q/daemon.toml.bak* %q %q %q %q 2>/dev/null || true\n' \
    "$DAEMON_USER" "$DAEMON_GROUP" \
    "$ETC_DIR/daemon.toml" "$ETC_DIR" \
    "$key" "$key.id" "$key.token" "$key.previous"
  printf 'sudo chmod 0640 %q %q/daemon.toml.bak* 2>/dev/null || true\n' \
    "$ETC_DIR/daemon.toml" "$ETC_DIR"
  printf 'sudo chmod 0600 %q %q %q %q 2>/dev/null || true' \
    "$key" "$key.id" "$key.token" "$key.previous"
}

PAIR_CMD="$(pair_command_hint)"
RESTORE_HINT="$(ownership_restore_hint)"

if [ "$INSTALL_SYSTEMD" -eq 1 ] && [ -z "$ROOT_DIR" ]; then
  ADMIN_IS_ACTIVE=0
  if command -v systemctl >/dev/null 2>&1 \
      && systemctl is-active --quiet fleet-admin 2>/dev/null; then
    ADMIN_IS_ACTIVE=1
  fi

  if [ "$ADMIN_IS_ACTIVE" -ne 1 ]; then
    echo "fleet-admin is not running yet; pair the daemon after starting it with:"
    echo "  $PAIR_CMD"
    echo "then restore ownership with:"
    echo "$RESTORE_HINT" | sed 's/^/  /'
  elif wait_for_admin "$ADMIN_PORT"; then
    echo "Pairing the daemon with the agent admin on 127.0.0.1:$ADMIN_PORT..."
    # The admin token is passed via the environment (never argv) and HOME is the
    # daemon's so `~` in an existing key_file resolves like it will for the
    # service.
    if HOME="$DAEMON_HOME" FLEET_ADMIN_TOKEN="$ADMIN_TOKEN_VAL" \
        "$BIN_DIR/fleet" --admin-url "http://127.0.0.1:$ADMIN_PORT" daemon setup \
        --system --no-install-service \
        --org org_default \
        --server "ws://127.0.0.1:$ADMIN_PORT/ws/machine" \
        --config "$ETC_DIR/daemon.toml"; then
      restore_daemon_ownership
      echo "daemon paired; the per-machine token is stored next to the machine key"
    else
      restore_daemon_ownership
      warn "automatic pairing failed; run: $PAIR_CMD"
      warn "then restore ownership with:"
      while IFS= read -r line; do warn "  $line"; done <<<"$RESTORE_HINT" || true
    fi
  else
    warn "fleet-admin did not become healthy on 127.0.0.1:$ADMIN_PORT within 30s; pair the daemon with:"
    warn "  $PAIR_CMD"
    warn "then restore ownership with:"
    while IFS= read -r line; do warn "  $line"; done <<<"$RESTORE_HINT" || true
  fi
fi

trap - ERR

if [ -n "$BACKUP_DEST" ]; then
  echo "previous binaries and units backed up to $BACKUP_DEST"
  echo "rollback: stop services, copy $BACKUP_DEST/*.service back to $SYSTEMD_DIR,"
  echo "          copy the binaries back to $BIN_DIR, then run systemctl daemon-reload"
fi

cat <<EOF

Installed:
  binaries  $BIN_DIR/fleet-admin, $BIN_DIR/fleet-daemon, $BIN_DIR/fleet
  web       $WEB_DIR
  config    $ETC_DIR (admin: $ADMIN_USER, daemon: $DAEMON_USER)
  state     admin $ADMIN_STATE_DIR, daemon $DAEMON_STATE_DIR
  backups   $BACKUP_DIR

Before starting:
  1. The admin token is in $ETC_DIR/admin.env (FLEET_ADMIN_TOKEN).
     Machines do not share a token: pair each daemon with a short-lived code.
     Start fleet-admin first if it is not running, then run:
       ${PAIR_CMD}
     After a manual run, restore file ownership with:
${RESTORE_HINT}
  2. The daemon runs as $DAEMON_USER and needs access to repositories and the
     agent CLIs (claude/codex/opencode) with their credentials.
  3. The admin binds loopback; use Tailscale/Cloudflare/reverse proxy for remote access.
EOF
