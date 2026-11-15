#!/usr/bin/env bash
# Git write end-to-end test: authorization, stage/unstage/commit/checkout and
# audit records against real binaries and a real repository.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CARGO="${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo}"
TOKEN="${AGENT_FLEET_E2E_TOKEN:-git-token}"
MACHINE_TOKEN="${AGENT_FLEET_E2E_MACHINE_TOKEN:-git-write-machine-token}"
WRITE_TOKEN="${AGENT_FLEET_E2E_WRITE_TOKEN:-git-write-secret}"
ADMIN_PORT="${E2E_ADMIN_PORT:-21787}"
DAEMON_PORT="${E2E_DAEMON_PORT:-48081}"
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
write_api() {
    curl -fsS -H "Authorization: Bearer $TOKEN" -H "X-Agent-Fleet-Git-Token: $WRITE_TOKEN" "$@"
}
status_of() { curl -sS -o "$2" -w '%{http_code}' -H "Authorization: Bearer $TOKEN" "${@:3}" "$1"; }

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

dump_logs() {
    echo '--- agent-admin.log (tail) ---' >&2
    tail -n 40 "$TMP/agent-admin.log" >&2 2>/dev/null || true
    echo '--- daemon.log (tail) ---' >&2
    tail -n 40 "$TMP/daemon.log" >&2 2>/dev/null || true
}

log "Building binaries"
cd "$ROOT"
CARGO_BUILD_JOBS=4 "$CARGO" build -q -p fleet-admin -p fleet-daemon
TARGET_DIR="$("$CARGO" metadata --no-deps --format-version 1 | jq -r .target_directory)"
ADMIN_BIN="$TARGET_DIR/debug/agent-admin"
DAEMON_BIN="$TARGET_DIR/debug/fleet-daemon"
[ -x "$ADMIN_BIN" ] || fail "agent-admin binary missing at $ADMIN_BIN"
[ -x "$DAEMON_BIN" ] || fail "daemon binary missing at $DAEMON_BIN"

log "Preparing a git workspace"
mkdir -p "$REPO/src"
git -C "$REPO" init -q
git -C "$REPO" config user.name "Git Write E2E"
git -C "$REPO" config user.email "git-write@example.com"
git -C "$REPO" config commit.gpgsign false
printf 'one\n' > "$REPO/a.txt"
printf 'keep\n' > "$REPO/b.txt"
printf 'fn main() {}\n' > "$REPO/src/main.rs"
git -C "$REPO" add -A
git -C "$REPO" commit -qm "initial"
BASE_BRANCH="$(git -C "$REPO" rev-parse --abbrev-ref HEAD)"
printf 'two\n' > "$REPO/a.txt"
printf 'new\n' > "$REPO/new.txt"

log "Starting agent admin on 127.0.0.1:$ADMIN_PORT with a dedicated write token"
AGENT_FLEET_GIT_WRITE_TOKEN="$WRITE_TOKEN" \
AGENT_FLEET_MACHINE_TOKEN="$MACHINE_TOKEN" \
"$ADMIN_BIN" serve \
    --bind "127.0.0.1:$ADMIN_PORT" \
    --db "$TMP/agent-admin.db" \
    --admin-token "$TOKEN" \
    > "$TMP/agent-admin.log" 2>&1 &
ADMIN_PID=$!

for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:$ADMIN_PORT/api/v1/health" >/dev/null 2>&1; then break; fi
    sleep 0.1
done
curl -fsS "http://127.0.0.1:$ADMIN_PORT/api/v1/health" | jq -e '.ok == true' >/dev/null \
    || { dump_logs; fail "agent admin health check"; }

log "Starting daemon on 127.0.0.1:$DAEMON_PORT"
mkdir -p "$TMP/data"
cat > "$TMP/daemon.toml" <<TOML
[machine]
name = "git-write-machine"
key_file = "$TMP/data/machine.key"

[daemon]
host = "127.0.0.1"
port = $DAEMON_PORT

[storage]
database = "$TMP/data/agent.db"

[agent]
default_provider = "mock"

[connection]
server_url = "ws://127.0.0.1:$ADMIN_PORT/ws/machine"
token = "$MACHINE_TOKEN"
heartbeat_interval_ms = 1000
reconnect_max_ms = 2000
TOML
"$DAEMON_BIN" --config "$TMP/daemon.toml" serve > "$TMP/daemon.log" 2>&1 &
DAEMON_PID=$!

MACHINE="$(wait_for "http://127.0.0.1:$ADMIN_PORT/api/v1/machines" \
    'length == 1 and .[0].status == "online"' "machine did not register")" \
    || { dump_logs; fail "machine did not register"; }
MACHINE_ID="$(echo "$MACHINE" | jq -r '.[0].id')"
WS="$(api -X POST -H 'Content-Type: application/json' \
    -d "{\"machine_id\":\"$MACHINE_ID\",\"root_path\":\"$REPO\",\"name\":\"git-write\"}" \
    "http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces")" || { dump_logs; fail "workspace registration"; }
WS_ID="$(echo "$WS" | jq -r .id)"
echo "machine_id=$MACHINE_ID workspace_id=$WS_ID"

STAGE_URL="http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/stage"
UNSTAGE_URL="http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/unstage"
COMMIT_URL="http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/commit"
CHECKOUT_URL="http://127.0.0.1:$ADMIN_PORT/api/v1/workspaces/$WS_ID/git/checkout"

log "Writes without the dedicated token are rejected"
code="$(status_of "$STAGE_URL" "$TMP/denied.json" -X POST -H 'Content-Type: application/json' \
    -d '{"paths":["a.txt"]}')"
[ "$code" = "401" ] || fail "stage without write token returned $code, expected 401"
jq -e '.code == "unauthorized"' "$TMP/denied.json" >/dev/null || fail "unauthorized payload missing"

log "Stage and unstage"
write_api -X POST -H 'Content-Type: application/json' -d '{"paths":["a.txt","new.txt"]}' \
    "$STAGE_URL" | jq -e '.ok == true' >/dev/null || fail "stage failed"
STAGED="$(git -C "$REPO" diff --cached --name-only)"
echo "$STAGED" | grep -qx 'a.txt' || fail "a.txt was not staged"
echo "$STAGED" | grep -qx 'new.txt' || fail "new.txt was not staged"

write_api -X POST -H 'Content-Type: application/json' -d '{"paths":["new.txt"]}' \
    "$UNSTAGE_URL" | jq -e '.ok == true' >/dev/null || fail "unstage failed"
STAGED="$(git -C "$REPO" diff --cached --name-only)"
echo "$STAGED" | grep -qx 'a.txt' || fail "a.txt lost its staged state"
if echo "$STAGED" | grep -qx 'new.txt'; then fail "new.txt is still staged"; fi

log "Commit only a.txt"
BEFORE_HEAD="$(git -C "$REPO" rev-parse HEAD)"
COMMIT="$(write_api -X POST -H 'Content-Type: application/json' \
    -d '{"message":"e2e: commit a","paths":["a.txt"]}' "$COMMIT_URL")"
echo "$COMMIT" | jq -e '.summary == "e2e: commit a"' >/dev/null || fail "commit summary mismatch"
AFTER_HEAD="$(git -C "$REPO" rev-parse HEAD)"
[ "$AFTER_HEAD" != "$BEFORE_HEAD" ] || fail "HEAD did not advance"
[ "$(echo "$COMMIT" | jq -r .commit)" = "$AFTER_HEAD" ] || fail "returned commit hash mismatch"
[ "$(git -C "$REPO" log -1 --pretty=%s)" = "e2e: commit a" ] || fail "commit message missing"
[ "$(echo "$COMMIT" | jq -r .branch)" = "$BASE_BRANCH" ] || fail "branch mismatch"
git -C "$REPO" show --stat --oneline HEAD | grep -q 'a.txt' || fail "commit does not contain a.txt"

log "Commit with nothing staged is a conflict"
code="$(curl -sS -o "$TMP/conflict.json" -w '%{http_code}' -X POST \
    -H "Authorization: Bearer $TOKEN" -H "X-Agent-Fleet-Git-Token: $WRITE_TOKEN" \
    -H 'Content-Type: application/json' -d '{"message":"noop"}' "$COMMIT_URL")"
[ "$code" = "409" ] || fail "empty commit returned $code, expected 409"
jq -e '.code == "conflict"' "$TMP/conflict.json" >/dev/null || fail "conflict payload missing"

log "Checkout: create a branch and switch back"
OUT="$(write_api -X POST -H 'Content-Type: application/json' \
    -d '{"branch":"feature/e2e","create":true}' "$CHECKOUT_URL")"
[ "$(echo "$OUT" | jq -r .branch)" = "feature/e2e" ] || fail "create checkout mismatch"
[ "$(git -C "$REPO" rev-parse --abbrev-ref HEAD)" = "feature/e2e" ] || fail "branch not switched"
OUT="$(write_api -X POST -H 'Content-Type: application/json' \
    -d "{\"branch\":\"$BASE_BRANCH\"}" "$CHECKOUT_URL")"
[ "$(echo "$OUT" | jq -r .branch)" = "$BASE_BRANCH" ] || fail "switch-back mismatch"

log "Invalid paths are rejected and audited"
code="$(status_of "$STAGE_URL" "$TMP/invalid.json" -X POST \
    -H "X-Agent-Fleet-Git-Token: $WRITE_TOKEN" -H 'Content-Type: application/json' \
    -d '{"paths":["../escape"]}')"
[ "$code" = "400" ] || fail "invalid path returned $code, expected 400"
jq -e '.code == "invalid_request"' "$TMP/invalid.json" >/dev/null || fail "invalid path payload"

log "Audit trail records every attempt"
AUDIT="$(api "http://127.0.0.1:$ADMIN_PORT/api/v1/audit?limit=100")"
for expected in \
    'select(.action == "git.stage" and .details.outcome == "denied")' \
    'select(.action == "git.stage" and .details.outcome == "ok")' \
    'select(.action == "git.unstage" and .details.outcome == "ok")' \
    'select(.action == "git.commit" and .details.outcome == "ok")' \
    'select(.action == "git.checkout" and .details.outcome == "ok")' \
    'select(.action == "git.commit" and .details.outcome == "error")' \
    'select(.action == "git.stage" and .details.outcome == "error")'; do
    echo "$AUDIT" | jq -e "[.entries[] | $expected] | length > 0" >/dev/null \
        || fail "missing audit record: $expected"
done

ENTRY="$(echo "$AUDIT" | jq -c '[.entries[] | select(.action == "git.commit" and .details.outcome == "ok")][0]')"
[ "$(echo "$ENTRY" | jq -r .subject)" = "$WS_ID" ] || fail "audit subject mismatch"
[ "$(echo "$ENTRY" | jq -r .actor)" = "admin" ] || fail "audit actor mismatch"
[ "$(echo "$ENTRY" | jq -r .details.commit)" = "$AFTER_HEAD" ] || fail "audit commit hash mismatch"

printf '\n\033[1;32mGIT WRITE E2E PASSED\033[0m\n'
