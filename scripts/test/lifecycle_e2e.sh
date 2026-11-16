#!/usr/bin/env bash
# Exercise the fallback supervisor without systemd or sudo.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PCARGO="${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo}"
TMP="$(mktemp -d)"
PORT="${FLEET_LIFECYCLE_TEST_PORT:-18988}"

cleanup() {
  local state="$TMP/admin/run/supervisor.json"
  if [ -f "$state" ]; then
    local pid
    pid="$(sed -n 's/.*"supervisor_pid": \([0-9]*\).*/\1/p' "$state" | head -1)"
    if [ -n "$pid" ]; then
      kill -TERM "$pid" 2>/dev/null || true
      for _ in $(seq 1 30); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
      done
      kill -KILL "$pid" 2>/dev/null || true
    fi
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

cd "$ROOT"
CARGO_BUILD_JOBS=4 "$PCARGO" build -p fleet-cli -p fleet-admin -p fleet-daemon >/dev/null

unset FLEET_ADMIN_TOKEN FLEET_CONFIG FLEET_DB FLEET_DATA_DIR FLEET_SERVER_URL
unset FLEET_PAIR_CODE FLEET_BIND FLEET_MACHINE_NAME FLEET_ADMIN_URL
unset FLEET_CLI_CONFIG FLEET_ADMIN_BIN FLEET_DAEMON_BIN
export HOME="$TMP/home"
export FLEET_CLI_CONFIG="$TMP/home/.fleet/cli.toml"
mkdir -p "$HOME" "$TMP/admin"

FLEET="$ROOT/target/debug/fleet"
ADMIN_CONFIG="$TMP/admin/config.toml"
ADMIN_STATE="$TMP/admin"
ADMIN_DB="$TMP/admin/fleet-admin.db"
STATE="$ADMIN_STATE/run/supervisor.json"

"$FLEET" admin --user setup \
  --yes --no-install-service --no-web \
  --config "$ADMIN_CONFIG" --state-dir "$ADMIN_STATE" --database "$ADMIN_DB" \
  --bind "127.0.0.1:$PORT" >/dev/null

"$FLEET" admin --user --config "$ADMIN_CONFIG" --state-dir "$ADMIN_STATE" \
  start --mode supervisor

test -f "$STATE"
test -f "$ADMIN_STATE/logs/admin.log"
test -f "$ADMIN_STATE/logs/supervisor.log"
grep -q '"supervisor_pid"' "$STATE"
grep -q '"child_pid"' "$STATE"

"$FLEET" --json admin --user --config "$ADMIN_CONFIG" --state-dir "$ADMIN_STATE" \
  status --mode supervisor >"$TMP/status.json"
grep -q '"mode": "supervisor"' "$TMP/status.json"
grep -q '"healthy": true' "$TMP/status.json"

"$FLEET" admin --user --config "$ADMIN_CONFIG" --state-dir "$ADMIN_STATE" \
  logs -n 20 --mode supervisor >/dev/null

OLD_PID="$(sed -n 's/.*"supervisor_pid": \([0-9]*\).*/\1/p' "$STATE" | head -1)"
test -n "$OLD_PID"

"$FLEET" admin --user --config "$ADMIN_CONFIG" --state-dir "$ADMIN_STATE" \
  restart --mode supervisor
NEW_PID="$(sed -n 's/.*"supervisor_pid": \([0-9]*\).*/\1/p' "$STATE" | head -1)"
test -n "$NEW_PID"
test "$OLD_PID" != "$NEW_PID"
grep -q '"restarts": 0' "$STATE"

"$FLEET" admin --user --config "$ADMIN_CONFIG" --state-dir "$ADMIN_STATE" \
  stop --mode supervisor
test ! -f "$STATE"

set +e
"$FLEET" admin --user --config "$ADMIN_CONFIG" --state-dir "$ADMIN_STATE" \
  status --mode supervisor >/dev/null 2>&1
CODE=$?
set -e
test "$CODE" -eq 1

curl -fsS "http://127.0.0.1:$PORT/api/v1/health" >/dev/null 2>&1 && {
  echo "admin still healthy after stop" >&2
  exit 1
} || true

echo "lifecycle e2e passed"
