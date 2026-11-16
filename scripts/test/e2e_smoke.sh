#!/usr/bin/env bash
# Black-box end-to-end smoke test:
#   fleet-admin + fleet-daemon (mock provider) + a real temp git repo.
#
# Requires: curl, jq, git, the prebuilt cargo below.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CARGO="${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo}"
TOKEN="${E2E_TOKEN:-e2e-token}"
ADMIN_PORT="${E2E_ADMIN_PORT:-18787}"
DAEMON_PORT="${E2E_DAEMON_PORT:-28080}"
TMP="$(mktemp -d)"
REPO="$TMP/repo"
ADMIN_PID=""
DAEMON_PID=""
PREPAIR_PID=""

cleanup() {
    set +e
    [ -n "$PREPAIR_PID" ] && kill -KILL "$PREPAIR_PID" 2>/dev/null
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
delete_ok() { # url -> true when the DELETE returns 2xx
    local code
    code="$(curl -sS -o /dev/null -w '%{http_code}' -X DELETE \
        -H "Authorization: Bearer $TOKEN" "$1")"
    case "$code" in 2*) return 0;; *) printf 'DELETE %s returned %s\n' "$1" "$code" >&2; return 1;; esac
}

wait_for() { # url, jq-expr, message
    local url="$1" expr="$2" msg="$3"
    for _ in $(seq 1 100); do
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

log "Building binaries"
cd "$ROOT"
CARGO_BUILD_JOBS=4 "$CARGO" build -q -p fleet-admin -p fleet-daemon -p fleet-cli
TARGET_DIR="$("$CARGO" metadata --no-deps --format-version 1 | jq -r .target_directory)"
ADMIN_BIN="$TARGET_DIR/debug/fleet-admin"
DAEMON_BIN="$TARGET_DIR/debug/fleet-daemon"
FLEET_BIN="$TARGET_DIR/debug/fleet"
[ -x "$ADMIN_BIN" ] || fail "fleet-admin binary missing at $ADMIN_BIN"
[ -x "$DAEMON_BIN" ] || fail "daemon binary missing at $DAEMON_BIN"
[ -x "$FLEET_BIN" ] || fail "fleet binary missing at $FLEET_BIN"

log "Preparing a git workspace"
mkdir -p "$REPO"
git -C "$REPO" init -q
git -C "$REPO" config user.name "E2E"
git -C "$REPO" config user.email "e2e@example.com"
printf 'hello\nworld\n' > "$REPO/a.txt"
mkdir -p "$REPO/src"
printf 'fn main() {}\n' > "$REPO/src/main.rs"
git -C "$REPO" add -A
git -C "$REPO" commit -qm "initial"
printf 'hello\nworld\nchanged\n' > "$REPO/a.txt"
printf 'untracked\n' > "$REPO/new.txt"

log "Starting fleet-admin on 127.0.0.1:$ADMIN_PORT"
FLEET_ADMIN_TOKEN="$TOKEN" \
FLEET_BIND="127.0.0.1:$ADMIN_PORT" \
FLEET_DB="$TMP/fleet-admin.db" \
"$ADMIN_BIN" serve > "$TMP/fleet-admin.log" 2>&1 &
ADMIN_PID=$!

for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:$ADMIN_PORT/api/v1/health" >/dev/null 2>&1; then break; fi
    sleep 0.1
done
curl -fsS "http://127.0.0.1:$ADMIN_PORT/api/v1/health" | jq -e '.ok == true' >/dev/null \
    || fail "fleet-admin health check"

log "An unpaired daemon is rejected"
# The literal error code is asserted by crates/core-connection/tests/gateway.rs
# (unpaired_machine_without_credentials_is_terminal); the client renders it as
# "machine is not paired" in its terminal-authentication log line.
mkdir -p "$TMP/prepair"
cat > "$TMP/prepair/daemon.toml" <<TOML
[machine]
name = "unpaired"
key_file = "$TMP/prepair/machine.key"

[daemon]
host = "127.0.0.1"
port = 0

[storage]
database = "$TMP/prepair/fleet-daemon.db"

[agent]
default_provider = "mock"

[connection]
server_url = "ws://127.0.0.1:$ADMIN_PORT/ws/machine"
heartbeat_interval_ms = 1000
reconnect_max_ms = 2000
TOML
RUST_LOG="fleet_daemon=info,fleet_core_connection=info" \
    "$DAEMON_BIN" --config "$TMP/prepair/daemon.toml" serve \
    > "$TMP/prepair/daemon.log" 2>&1 &
PREPAIR_PID=$!
for _ in $(seq 1 100); do
    grep -q "machine is not paired" "$TMP/prepair/daemon.log" && break
    sleep 0.1
done
kill -INT "$PREPAIR_PID" 2>/dev/null || true
sleep 0.3
kill -KILL "$PREPAIR_PID" 2>/dev/null || true
wait "$PREPAIR_PID" 2>/dev/null || true
PREPAIR_PID=""
grep -q "machine is not paired" "$TMP/prepair/daemon.log" \
    || fail "unpaired daemon was not rejected with machine_not_paired"
grep -q "machine_not_paired" "$TMP/prepair/daemon.log" \
    || fail "unpaired daemon log did not include the literal machine_not_paired code"

log "Pairing the daemon"
mkdir -p "$TMP/data"
cat > "$TMP/daemon.toml" <<TOML
[machine]
name = "e2e-machine"
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
"$FLEET_BIN" --admin-url "http://127.0.0.1:$ADMIN_PORT" --admin-token "$TOKEN" daemon setup --no-install-service \
    --org org_default \
    --server "ws://127.0.0.1:$ADMIN_PORT/ws/machine" \
    --config "$TMP/daemon.toml" \
    --name "e2e-machine"
FLEET_TOKEN_FILE="$TMP/data/machine.key.token"
[ -f "$FLEET_TOKEN_FILE" ] || fail "pairing did not write $FLEET_TOKEN_FILE"
TOKEN_MODE="$(stat -c '%a' "$FLEET_TOKEN_FILE" 2>/dev/null || stat -f '%Lp' "$FLEET_TOKEN_FILE")"
[ "$TOKEN_MODE" = "600" ] || fail "machine token mode is $TOKEN_MODE, expected 600"
if grep -Eq '^[[:space:]]*token[[:space:]]*=' "$TMP/daemon.toml"; then
    fail "daemon config still contains a machine token"
fi

log "Starting daemon on 127.0.0.1:$DAEMON_PORT"
"$DAEMON_BIN" --config "$TMP/daemon.toml" serve > "$TMP/daemon.log" 2>&1 &
DAEMON_PID=$!

log "Waiting for machine to come online"
MACHINE="$(wait_for "http://127.0.0.1:$ADMIN_PORT/api/v1/machines" 'length == 1 and .[0].status == "online"' "machine did not register")" || fail "machine did not register"
MACHINE_ID="$(echo "$MACHINE" | jq -r '.[0].id')"
echo "machine_id=$MACHINE_ID"

log "Providers"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/machines/$MACHINE_ID/providers" \
    | jq -e 'map(.id) | index("mock") != null' >/dev/null || fail "mock provider missing"

log "Registering workspace"
WS="$(api -X POST -H 'Content-Type: application/json' \
    -d "{\"machine_id\":\"$MACHINE_ID\",\"root_path\":\"$REPO\",\"name\":\"e2e\"}" \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces")"
WS_ID="$(echo "$WS" | jq -r .id)"
echo "workspace_id=$WS_ID"

log "Directory listing"
TREE="$(api "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/tree?path=")"
echo "$TREE" | jq -e '[.entries[].name] | index("a.txt") != null and index("src") != null' >/dev/null \
    || fail "tree missing entries: $TREE"
echo "$TREE" | jq -e '.entries[0].entry_type == "directory"' >/dev/null \
    || fail "directories must sort first"

log "File content (+ ETag)"
FILE="$(api "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/files?path=a.txt")"
echo "$FILE" | jq -e '.content | contains("changed")' >/dev/null || fail "file content mismatch"
ETAG="$(echo "$FILE" | jq -r .etag)"
STATUS="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN" \
    -H "If-None-Match: $ETAG" \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/files?path=a.txt&etag=$ETAG")"
[ "$STATUS" = "304" ] || fail "expected 304 not modified, got $STATUS"

log "Git status / diff summary / diff"
GIT_STATUS="$(api "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/status")"
echo "$GIT_STATUS" | jq -e '[.files[].path] | index("a.txt") != null and index("new.txt") != null' >/dev/null \
    || fail "git status missing changes: $GIT_STATUS"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/diff/summary" \
    | jq -e '[.files[].path] | index("a.txt") != null' >/dev/null || fail "diff summary missing a.txt"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/diff?path=a.txt&include_untracked=true" \
    | jq -e '.files[0].hunks | length > 0' >/dev/null || fail "no diff hunks for a.txt"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/branches" | jq -e 'length > 0' >/dev/null \
    || fail "no branches"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/log" | jq -e 'length > 0' >/dev/null \
    || fail "no log entries"

log "Creating a mock agent session"
SESSION="$(api -X POST -H 'Content-Type: application/json' \
    -d "{\"machine_id\":\"$MACHINE_ID\",\"provider\":\"mock\",\"workspace_id\":\"$WS_ID\",\"title\":\"e2e\"}" \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions")"
SESSION_ID="$(echo "$SESSION" | jq -r .id)"
echo "session_id=$SESSION_ID"

log "Sending a message and waiting for the streamed echo"
api -X POST -H 'Content-Type: application/json' -d '{"content":"hello e2e"}' \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID/messages" | jq -e '.ok == true' >/dev/null
DETAIL="$(wait_for "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID" \
    '[.messages[]?.content] | join("") | contains("Echo: hello e2e")' "no echoed assistant message")" || fail "no echoed assistant message"
echo "$DETAIL" | jq -r '.messages[]?.content' | tail -2

log "Permission approval flow"
api -X POST -H 'Content-Type: application/json' -d '{"content":"/tool"}' \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID/messages" >/dev/null
PERM="$(wait_for "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID/permissions" \
    'length > 0 and .[0].status == "pending"' "no pending permission")" || fail "no pending permission"
PERM_ID="$(echo "$PERM" | jq -r '.[0].id')"
echo "permission_id=$PERM_ID tool=$(echo "$PERM" | jq -r '.[0].tool_name')"
api -X POST -H 'Content-Type: application/json' -d '{}' \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/permissions/$PERM_ID/approve" \
    | jq -e '.status == "approved" or .ok == true' >/dev/null || fail "permission approve failed"
wait_for "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID" \
    '[.messages[]?.content] | join("") | contains("Tool finished")' "tool completion missing" >/dev/null || fail "tool completion missing"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID" \
    | jq -e '[.tool_calls[]? | select(.name == "Bash" and .status == "completed" and .output.stdout == "hello\n")] | length == 1' >/dev/null \
    || fail "tool call history missing"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID" \
    | jq -e '[.messages[]? | select(.role == "user" and .content == "/tool")] | length == 1' >/dev/null \
    || fail "user message history missing"

log "Interrupt flow"
SESSION2="$(api -X POST -H 'Content-Type: application/json' \
    -d "{\"machine_id\":\"$MACHINE_ID\",\"provider\":\"mock\",\"workspace_id\":\"$WS_ID\",\"title\":\"interrupt\"}" \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions")"
SESSION2_ID="$(echo "$SESSION2" | jq -r .id)"
api -X POST -H 'Content-Type: application/json' -d '{"content":"/slow"}' \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION2_ID/messages" >/dev/null
sleep 0.6
api -X POST "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION2_ID/interrupt" | jq -e '.ok == true' >/dev/null
wait_for "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION2_ID" \
    '.session.status == "interrupted" or .session.status == "waiting_for_input"' "session not interrupted" >/dev/null || fail "session not interrupted"

log "Durable events are acknowledged and pruned"
OUTBOX_ROWS=""
OUTBOX_CURSOR=0
for _ in $(seq 1 100); do
    OUTBOX_ROWS="$(sqlite3 "$TMP/data/fleet-daemon.db" 'SELECT COUNT(*) FROM event_outbox;' 2>/dev/null || true)"
    OUTBOX_CURSOR="$(sqlite3 "$TMP/data/fleet-daemon.db" "SELECT acked_sequence FROM event_cursors WHERE consumer = 'admin';" 2>/dev/null || echo 0)"
    if [ "$OUTBOX_ROWS" = "0" ] && [ "${OUTBOX_CURSOR:-0}" -gt 0 ]; then
        break
    fi
    sleep 0.2
done
[ "$OUTBOX_ROWS" = "0" ] || fail "daemon outbox still holds $OUTBOX_ROWS unacknowledged events"
[ "${OUTBOX_CURSOR:-0}" -gt 0 ] || fail "admin cursor did not advance"

log "Workspace deletion is blocked while sessions are active"
code="$(curl -sS -o /dev/null -w '%{http_code}' -X DELETE     -H "Authorization: Bearer $TOKEN"     "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID")"
[ "$code" = "409" ] || fail "expected 409 deleting a busy workspace, got $code"

log "Close archives a session and keeps history"
api -X POST "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID/close" >/dev/null
wait_for "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID"     '.session.status == "terminated"' "session did not archive" >/dev/null || fail "session did not archive"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions"     | jq -e --arg id "$SESSION_ID" 'map(.id) | index($id) == null' >/dev/null     || fail "archived session leaked into the default list"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions?include_closed=true"     | jq -e --arg id "$SESSION_ID" 'map(select(.id == $id and .status == "terminated")) | length == 1' >/dev/null     || fail "archived session missing from include_closed list"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID"     | jq -e '[.messages[]?.content] | join("") | contains("Echo: hello e2e")' >/dev/null     || fail "archived session lost its history"

log "Permanent delete removes the session and history"
delete_ok "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION_ID" || fail "session delete failed"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions?include_closed=true"     | jq -e --arg id "$SESSION_ID" 'map(.id) | index($id) == null' >/dev/null     || fail "deleted session still appears"
delete_ok "http://127.0.0.1:$ADMIN_PORT/api/v1/sessions/$SESSION2_ID" || fail "session2 delete failed"
delete_ok "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID" || fail "workspace delete failed"

log "Revoking a machine prevents it from re-pairing"
delete_ok "http://127.0.0.1:$ADMIN_PORT/api/v1/machines/$MACHINE_ID" || fail "machine revoke failed"
sleep 1
api "http://127.0.0.1:$ADMIN_PORT/api/v1/machines" \
    | jq -e --arg id "$MACHINE_ID" 'map(.id) | index($id) == null' >/dev/null \
    || fail "revoked machine came back"
api "http://127.0.0.1:$ADMIN_PORT/api/v1/machines/revoked" \
    | jq -e --arg id "$MACHINE_ID" 'map(.machine_id) | index($id) != null' >/dev/null \
    || fail "revoked machine missing from the revocation list"
api -X POST -H 'Content-Type: application/json' -d '{}' \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/machines/$MACHINE_ID/unrevoke" \
    | jq -e '.ok == true' >/dev/null || fail "machine unrevoke failed"

printf '\n\033[1;32mE2E SMOKE PASSED\033[0m\n'
