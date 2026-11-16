#!/usr/bin/env bash
# Exercise fleet admin/daemon setup without sudo, systemd, or the developer's
# real ~/.fleet. The admin server is started directly for the health probe.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PCARGO="${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo}"
TMP="$(mktemp -d)"
PORT="${FLEET_SETUP_TEST_PORT:-18987}"
ADMIN_PID=""
cleanup() {
  if [ -n "$ADMIN_PID" ]; then
    kill "$ADMIN_PID" 2>/dev/null || true
    wait "$ADMIN_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

# Build before replacing HOME; rustup needs the real toolchain home.
cd "$ROOT"
CARGO_BUILD_JOBS=4 "$PCARGO" build -p fleet-cli -p fleet-admin -p fleet-daemon >/dev/null

# Keep the test hermetic: no ambient FLEET_* overrides and no real home writes.
unset FLEET_ADMIN_TOKEN FLEET_CONFIG FLEET_DB FLEET_DATA_DIR FLEET_SERVER_URL
unset FLEET_PAIR_CODE FLEET_BIND FLEET_MACHINE_NAME FLEET_ADMIN_URL
unset FLEET_CLI_CONFIG
export HOME="$TMP/home"
export FLEET_CLI_CONFIG="$TMP/home/.fleet/cli.toml"
mkdir -p "$HOME" "$TMP/admin" "$TMP/daemon"

FLEET="$ROOT/target/debug/fleet"
ADMIN_CONFIG="$TMP/admin/config.toml"
ADMIN_STATE="$TMP/admin"
ADMIN_DB="$TMP/admin/fleet-admin.db"
DAEMON_CONFIG="$TMP/daemon/config.toml"
DAEMON_STATE="$TMP/daemon"
DAEMON_DB="$TMP/daemon/fleet-daemon.db"

"$FLEET" admin setup \
  --yes --no-install-service --no-web \
  --config "$ADMIN_CONFIG" --state-dir "$ADMIN_STATE" --database "$ADMIN_DB" \
  --bind "127.0.0.1:$PORT"

test -f "$ADMIN_CONFIG"
test -f "$FLEET_CLI_CONFIG"
grep -q 'admin_token = "fla_' "$ADMIN_CONFIG"
grep -q "database = \"$ADMIN_DB\"" "$ADMIN_CONFIG"

"$ROOT/target/debug/fleet-admin" --config "$ADMIN_CONFIG" serve >"$TMP/admin.log" 2>&1 &
ADMIN_PID=$!

for _ in $(seq 1 50); do
  if curl -fsS "http://127.0.0.1:$PORT/api/v1/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
curl -fsS "http://127.0.0.1:$PORT/api/v1/health" >/dev/null

TOKEN="$(sed -n 's/^admin_token = "\(.*\)"/\1/p' "$ADMIN_CONFIG" | head -1)"
test -n "$TOKEN"

"$FLEET" --admin-url "http://127.0.0.1:$PORT" --admin-token "$TOKEN" \
  daemon setup --yes --no-install-service --configure-only \
  --config "$DAEMON_CONFIG" --state-dir "$DAEMON_STATE" --database "$DAEMON_DB" \
  --server "ws://127.0.0.1:$PORT/ws/machine" --name setup-test

test -f "$DAEMON_CONFIG"
grep -q 'server_url = "ws://127.0.0.1:' "$DAEMON_CONFIG"
grep -q "database = \"$DAEMON_DB\"" "$DAEMON_CONFIG"
! grep -q 'pairing_code' "$DAEMON_CONFIG"
! grep -q 'token =' "$DAEMON_CONFIG"

"$FLEET" admin config validate --config "$ADMIN_CONFIG"
"$FLEET" daemon config validate --config "$DAEMON_CONFIG"
"$FLEET" daemon identity show --config "$DAEMON_CONFIG" >"$TMP/identity.json"
grep -q '"paired": false' "$TMP/identity.json"

# Service planning must render fleet ... run and never contain placeholders.
"$FLEET" admin install-service --dry-run --config "$ADMIN_CONFIG" >"$TMP/admin-service.txt"
"$FLEET" daemon install-service --dry-run --config "$DAEMON_CONFIG" >"$TMP/daemon-service.txt"
grep -q 'admin run --config' "$TMP/admin-service.txt"
grep -q 'daemon run --config' "$TMP/daemon-service.txt"
! grep -q '@[A-Z_]*@' "$TMP/admin-service.txt"
! grep -q '@[A-Z_]*@' "$TMP/daemon-service.txt"

echo "setup e2e passed"
