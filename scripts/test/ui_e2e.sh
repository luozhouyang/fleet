#!/usr/bin/env bash
# Browser-level end-to-end test:
#   fleet-admin (serving the built web UI) + fleet-daemon (mock provider) +
#   a real temp git repo, driven by Playwright/Chromium.
#
# Requires: curl, jq, git, pnpm, Chromium for Playwright, and the prebuilt
# cargo below. Run `pnpm --filter @agent-fleet/web exec playwright install
# chromium` once beforehand.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CARGO="${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo}"
TOKEN="${FLEET_E2E_TOKEN:-dev-token}"
ADMIN_PORT="${E2E_ADMIN_PORT:-18787}"
DAEMON_PORT="${E2E_DAEMON_PORT:-28080}"
TMP="$(mktemp -d)"
REPO="$TMP/repo"
ADMIN_PID=""
DAEMON_PID=""

cleanup() {
    set +e
    [ -n "$DAEMON_PID" ] && kill -INT "$DAEMON_PID" 2>/dev/null
    [ -n "$ADMIN_PID" ] && kill -INT "$ADMIN_PID" 2>/dev/null
    sleep 0.3
    [ -n "$DAEMON_PID" ] && kill -KILL "$DAEMON_PID" 2>/dev/null
    [ -n "$ADMIN_PID" ] && kill -KILL "$ADMIN_PID" 2>/dev/null
    if [ "${E2E_KEEP:-0}" != "1" ]; then rm -rf "$TMP"; else echo "artifacts kept in $TMP"; fi
}
trap cleanup EXIT

log() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31mFAIL: %s\033[0m\n' "$*"; exit 1; }
api() { curl -fsS -H "Authorization: Bearer $TOKEN" "$@"; }

dump_logs() {
    echo '--- fleet-admin.log (tail) ---' >&2
    tail -n 40 "$TMP/fleet-admin.log" >&2 2>/dev/null || true
    echo '--- daemon.log (tail) ---' >&2
    tail -n 40 "$TMP/daemon.log" >&2 2>/dev/null || true
}

wait_for() { # url, jq-expr, message
    local url="$1" expr="$2" msg="$3"
    for _ in $(seq 1 150); do
        if value="$(curl -fsS -H "Authorization: Bearer $TOKEN" "$url" 2>/dev/null)" \
            && echo "$value" | jq -e "$expr" >/dev/null 2>&1; then
            echo "$value"
            return 0
        fi
        sleep 0.2
    done
    printf 'wait_for failed: %s\n' "$msg" >&2
    return 1
}

log "Building Rust binaries"
cd "$ROOT"
CARGO_BUILD_JOBS=4 "$CARGO" build -q -p fleet-admin -p fleet-daemon -p fleet-cli
TARGET_DIR="$("$CARGO" metadata --no-deps --format-version 1 | jq -r .target_directory)"
ADMIN_BIN="$TARGET_DIR/debug/fleet-admin"
DAEMON_BIN="$TARGET_DIR/debug/fleet-daemon"
FLEET_BIN="$TARGET_DIR/debug/fleet"
[ -x "$ADMIN_BIN" ] || fail "fleet-admin binary missing at $ADMIN_BIN"
[ -x "$DAEMON_BIN" ] || fail "daemon binary missing at $DAEMON_BIN"
[ -x "$FLEET_BIN" ] || fail "fleet binary missing at $FLEET_BIN"

log "Building the web UI"
pnpm --filter @agent-fleet/web build >/dev/null
[ -f "$ROOT/apps/web/dist/index.html" ] || fail "apps/web/dist/index.html was not produced"

log "Preparing a git workspace"
mkdir -p "$REPO/src"
git -C "$REPO" init -q
git -C "$REPO" config user.name "UI E2E"
git -C "$REPO" config user.email "ui-e2e@example.com"
printf 'hello\nworld\n' > "$REPO/a.txt"
printf 'fn main() {\n    println!("hello world");\n}\n' > "$REPO/src/main.rs"
printf 'committed notes\n' > "$REPO/notes.txt"
git -C "$REPO" add -A
git -C "$REPO" commit -qm "initial"
printf 'hello\nworld\nchanged\n' > "$REPO/a.txt"
printf 'untracked\n' > "$REPO/new.txt"

log "Starting fleet-admin on 127.0.0.1:$ADMIN_PORT"
"$ADMIN_BIN" serve \
    --bind "127.0.0.1:$ADMIN_PORT" \
    --db "$TMP/fleet-admin.db" \
    --admin-token "$TOKEN" \
    --web-dir "$ROOT/apps/web/dist" \
    > "$TMP/fleet-admin.log" 2>&1 &
ADMIN_PID=$!

for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:$ADMIN_PORT/api/v1/health" >/dev/null 2>&1; then break; fi
    sleep 0.1
done
curl -fsS "http://127.0.0.1:$ADMIN_PORT/api/v1/health" | jq -e '.ok == true' >/dev/null \
    || { dump_logs; fail "fleet-admin health check"; }

log "Pairing the daemon"
mkdir -p "$TMP/data"
cat > "$TMP/daemon.toml" <<TOML
[machine]
name = "ui-e2e-machine"
key_file = "$TMP/data/machine.key"

[daemon]
host = "127.0.0.1"
port = $DAEMON_PORT

[storage]
database = "$TMP/data/fleet-daemon.db"

[agent]
default_provider = "mock"

[connection]
server_url = "ws://127.0.0.1:$ADMIN_PORT/ws/machine"
heartbeat_interval_ms = 1000
reconnect_max_ms = 2000
TOML
"$FLEET_BIN" --admin-url "http://127.0.0.1:$ADMIN_PORT" --admin-token "$TOKEN" daemon setup \
    --org org_default \
    --server "ws://127.0.0.1:$ADMIN_PORT/ws/machine" \
    --daemon-config "$TMP/daemon.toml" \
    --name "ui-e2e-machine" \
    || { dump_logs; fail "pairing failed"; }

log "Starting daemon on 127.0.0.1:$DAEMON_PORT"
"$DAEMON_BIN" --config "$TMP/daemon.toml" serve > "$TMP/daemon.log" 2>&1 &
DAEMON_PID=$!

log "Waiting for machine to come online"
MACHINE="$(wait_for "http://127.0.0.1:$ADMIN_PORT/api/v1/machines" \
    'length == 1 and .[0].status == "online"' "machine did not register")" \
    || { dump_logs; fail "machine did not register"; }
MACHINE_ID="$(echo "$MACHINE" | jq -r '.[0].id')"
echo "machine_id=$MACHINE_ID"

log "Registering workspace"
WS="$(api -X POST -H 'Content-Type: application/json' \
    -d "{\"machine_id\":\"$MACHINE_ID\",\"root_path\":\"$REPO\",\"name\":\"demo\"}" \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces")" || { dump_logs; fail "workspace registration"; }
WS_ID="$(echo "$WS" | jq -r .id)"
[ "$WS_ID" != "null" ] && [ -n "$WS_ID" ] || { dump_logs; fail "workspace id missing"; }
echo "workspace_id=$WS_ID"

# Let the workspace watcher settle before the UI asserts on Git state.
sleep 0.5

log "Running Playwright"
cd "$ROOT/apps/web"
set +e
E2E_BASE_URL="http://127.0.0.1:$ADMIN_PORT" \
E2E_TOKEN="$TOKEN" \
E2E_REPO="$REPO" \
pnpm exec playwright test "$@"
STATUS=$?
set -e

if [ "$STATUS" -ne 0 ]; then
    dump_logs
fi

exit "$STATUS"
