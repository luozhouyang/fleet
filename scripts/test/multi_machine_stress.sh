#!/usr/bin/env bash
# Multi-machine / multi-workspace stress and stability test.
#
# Topology: 1 control plane + 3 daemons (mock provider), 2 git workspaces per
# machine (6 total). The script exercises concurrent workspace reads, session
# creation/streaming, permission approvals and interrupts, then verifies that
# one machine going offline does not disturb the others and that it recovers
# with its workspaces after a restart.
#
# Requires: curl, jq, git, and the prebuilt cargo below.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CARGO="${PCARGO:-/home/luozhouyang/Applications/pcargo/cargo}"
TOKEN="${AGENT_FLEET_E2E_TOKEN:-stress-token}"
CP_PORT="${E2E_CP_PORT:-19787}"
BASE_DAEMON_PORT="${E2E_DAEMON_PORT:-38081}"
MACHINES=3
WORKSPACES_PER_MACHINE=2
WORKSPACES=$((MACHINES * WORKSPACES_PER_MACHINE))
TMP="$(mktemp -d)"
CP_PID=""
declare -a DAEMON_PID=()
declare -a MACHINE_ID=()
declare -a WS_ID=()
declare -a SESSION_ID=()

cleanup() {
    set +e
    for pid in "${DAEMON_PID[@]}"; do
        [ -n "$pid" ] && kill -INT "$pid" 2>/dev/null
    done
    [ -n "$CP_PID" ] && kill -INT "$CP_PID" 2>/dev/null
    sleep 0.5
    for pid in "${DAEMON_PID[@]}"; do
        [ -n "$pid" ] && kill -KILL "$pid" 2>/dev/null
    done
    [ -n "$CP_PID" ] && kill -KILL "$CP_PID" 2>/dev/null
    if [ "${E2E_KEEP:-0}" != "1" ]; then rm -rf "$TMP"; else echo "artifacts kept in $TMP"; fi
}
trap cleanup EXIT

log() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31mFAIL: %s\033[0m\n' "$*"; exit 1; }
api() { curl -fsS -H "Authorization: Bearer $TOKEN" "$@"; }
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
    for i in $(seq 0 $((MACHINES - 1))); do
        echo "--- daemon$i.log (tail) ---" >&2
        tail -n 20 "$TMP/daemon$i.log" >&2 2>/dev/null || true
    done
    echo '--- control-plane.log (tail) ---' >&2
    tail -n 20 "$TMP/control-plane.log" >&2 2>/dev/null || true
}

# Run every named job in parallel and fail if any of them fails.
parallel() { # name...
    local failed=0 job
    local -a pids=()
    for job in "$@"; do
        ( eval "$job" ) &
        pids+=("$!")
    done
    for pid in "${pids[@]}"; do
        wait "$pid" || failed=1
    done
    [ "$failed" -eq 0 ]
}

session_detail() { api "http://127.0.0.1:$CP_PORT/api/v1/sessions/$1"; }
machine_by_index() {
    api "http://127.0.0.1:$CP_PORT/api/v1/machines" \
        | jq -c --arg id "${MACHINE_ID[$1]}" 'map(select(.id == $id))[0]'
}
online_count() {
    api "http://127.0.0.1:$CP_PORT/api/v1/machines" \
        | jq '[.[] | select(.status == "online")] | length'
}

start_daemon() { # index
    local i="$1"
    mkdir -p "$TMP/data$i"
    cat > "$TMP/daemon$i.toml" <<TOML
[machine]
name = "stress-m$i"
key_file = "$TMP/data$i/machine.key"

[daemon]
host = "127.0.0.1"
port = $((BASE_DAEMON_PORT + i))

[storage]
database = "$TMP/data$i/agent.db"

[agent]
default_provider = "mock"

[connection]
server_url = "ws://127.0.0.1:$CP_PORT/ws/machine"
token = "$TOKEN"
heartbeat_interval_ms = 500
reconnect_max_ms = 1000
TOML
    "$DAEMON_BIN" --config "$TMP/daemon$i.toml" serve \
        > "$TMP/daemon$i.log" 2>&1 &
    DAEMON_PID[$i]=$!
}

create_session() { # workspace index
    local i="$1"
    local machine=$((i / WORKSPACES_PER_MACHINE))
    api -X POST -H 'Content-Type: application/json' \
        -d "{\"machine_id\":\"${MACHINE_ID[$machine]}\",\"provider\":\"mock\",\"workspace_id\":\"${WS_ID[$i]}\",\"title\":\"stress-$i\"}" \
        "http://127.0.0.1:$CP_PORT/api/v1/sessions" \
        | jq -e '.id != null' >/dev/null
}

send_message() { # workspace index, content
    api -X POST -H 'Content-Type: application/json' \
        -d "{\"content\":\"$2\"}" \
        "http://127.0.0.1:$CP_PORT/api/v1/sessions/${SESSION_ID[$1]}/messages" >/dev/null
}

wait_echo() { # workspace index
    local i="$1"
    wait_for "http://127.0.0.1:$CP_PORT/api/v1/sessions/${SESSION_ID[$i]}" \
        "[.messages[]?.content] | join(\"\") | contains(\"Echo: hello-w$i\")" \
        "session $i did not echo" >/dev/null
}

approve_pending() { # workspace index
    local i="$1"
    local perm
    perm="$(wait_for "http://127.0.0.1:$CP_PORT/api/v1/sessions/${SESSION_ID[$i]}/permissions" \
        'length > 0 and .[0].status == "pending"' "no pending permission for session $i")" \
        || return 1
    local perm_id
    perm_id="$(echo "$perm" | jq -r '.[0].id')"
    api -X POST -H 'Content-Type: application/json' -d '{}' \
        "http://127.0.0.1:$CP_PORT/api/v1/permissions/$perm_id/approve" >/dev/null
    wait_for "http://127.0.0.1:$CP_PORT/api/v1/sessions/${SESSION_ID[$i]}" \
        '[.messages[]?.content] | join("") | contains("Tool finished.")' \
        "tool did not finish for session $i" >/dev/null
}

interrupt_session() { # workspace index
    api -X POST "http://127.0.0.1:$CP_PORT/api/v1/sessions/${SESSION_ID[$1]}/interrupt" >/dev/null
    wait_for "http://127.0.0.1:$CP_PORT/api/v1/sessions/${SESSION_ID[$1]}" \
        '.session.status == "interrupted" or .session.status == "waiting_for_input"' \
        "session $1 was not interrupted" >/dev/null
}

tree_check() { # workspace index
    local entries
    entries="$(api "http://127.0.0.1:$CP_PORT/api/v1/workspaces/${WS_ID[$1]}/tree?path=")" \
        || return 1
    echo "$entries" | jq -e '[.entries[].name] | index("owner.txt") != null' >/dev/null
}

file_check() { # workspace index
    local content
    content="$(api "http://127.0.0.1:$CP_PORT/api/v1/workspaces/${WS_ID[$1]}/files?path=owner.txt")" \
        || return 1
    echo "$content" | jq -e --arg expected "machine-$(( $1 / WORKSPACES_PER_MACHINE )) workspace-$1" \
        '.content | contains($expected)' >/dev/null
}

# ---------------------------------------------------------------------------

log "Building binaries"
cd "$ROOT"
CARGO_BUILD_JOBS=4 "$CARGO" build -q -p agent-fleet-control-plane -p agent-fleet-daemon
TARGET_DIR="$("$CARGO" metadata --no-deps --format-version 1 | jq -r .target_directory)"
CP_BIN="$TARGET_DIR/debug/agent-fleet-control-plane"
DAEMON_BIN="$TARGET_DIR/debug/agent-fleet-daemon"
[ -x "$CP_BIN" ] || fail "control-plane binary missing at $CP_BIN"
[ -x "$DAEMON_BIN" ] || fail "daemon binary missing at $DAEMON_BIN"

log "Preparing $WORKSPACES git workspaces"
for i in $(seq 0 $((WORKSPACES - 1))); do
    repo="$TMP/repo$i"
    mkdir -p "$repo/src"
    git -C "$repo" init -q
    git -C "$repo" config user.name "Stress"
    git -C "$repo" config user.email "stress@example.com"
    printf 'machine-%s workspace-%s\n' "$((i / WORKSPACES_PER_MACHINE))" "$i" > "$repo/owner.txt"
    printf 'fn main() {}\n' > "$repo/src/main.rs"
    git -C "$repo" add -A
    git -C "$repo" commit -qm "initial"
    printf 'changed\n' >> "$repo/owner.txt"
done

log "Starting control plane on 127.0.0.1:$CP_PORT"
"$CP_BIN" serve \
    --bind "127.0.0.1:$CP_PORT" \
    --db "$TMP/control-plane.db" \
    --admin-token "$TOKEN" \
    > "$TMP/control-plane.log" 2>&1 &
CP_PID=$!

for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:$CP_PORT/api/v1/health" >/dev/null 2>&1; then break; fi
    sleep 0.1
done
curl -fsS "http://127.0.0.1:$CP_PORT/api/v1/health" | jq -e '.ok == true' >/dev/null \
    || { dump_logs; fail "control plane health check"; }

log "Starting $MACHINES daemons"
for i in $(seq 0 $((MACHINES - 1))); do start_daemon "$i"; done

wait_for "http://127.0.0.1:$CP_PORT/api/v1/machines" \
    "length == $MACHINES and (map(.status) | all(. == \"online\"))" \
    "all machines did not come online" > /dev/null || { dump_logs; fail "machines did not register"; }
MACHINES_JSON="$(api "http://127.0.0.1:$CP_PORT/api/v1/machines")"
for i in $(seq 0 $((MACHINES - 1))); do
    MACHINE_ID[$i]="$(echo "$MACHINES_JSON" | jq -r --arg name "stress-m$i" '.[] | select(.name == $name) | .id')"
    [ -n "${MACHINE_ID[$i]}" ] && [ "${MACHINE_ID[$i]}" != "null" ] || fail "machine stress-m$i missing"
    echo "machine[$i]=${MACHINE_ID[$i]}"
done

log "Registering $WORKSPACES workspaces concurrently"
register_workspace() { # workspace index
    local i="$1"
    local machine=$((i / WORKSPACES_PER_MACHINE))
    api -X POST -H 'Content-Type: application/json' \
        -d "{\"machine_id\":\"${MACHINE_ID[$machine]}\",\"root_path\":\"$TMP/repo$i\",\"name\":\"stress-w$i\"}" \
        "http://127.0.0.1:$CP_PORT/api/v1/workspaces" \
        | jq -er '.id' > "$TMP/ws$i.id"
}
JOBS=(); for i in $(seq 0 $((WORKSPACES - 1))); do JOBS+=("register_workspace $i"); done
parallel "${JOBS[@]}" || { dump_logs; fail "concurrent workspace registration"; }
for i in $(seq 0 $((WORKSPACES - 1))); do
    WS_ID[$i]="$(cat "$TMP/ws$i.id")"
    echo "workspace[$i]=${WS_ID[$i]}"
done
sleep 0.5

log "Concurrent tree/file reads across all workspaces"
JOBS=()
for round in 1 2 3; do
    for i in $(seq 0 $((WORKSPACES - 1))); do
        JOBS+=("tree_check $i" "file_check $i")
    done
done
parallel "${JOBS[@]}" || fail "concurrent workspace reads failed"
echo "ok: 36 concurrent tree/file requests"

log "Creating one session per workspace concurrently"
JOBS=(); for i in $(seq 0 $((WORKSPACES - 1))); do JOBS+=("create_session $i"); done
parallel "${JOBS[@]}" || { dump_logs; fail "concurrent session creation"; }
SESSIONS_JSON="$(api "http://127.0.0.1:$CP_PORT/api/v1/sessions")"
for i in $(seq 0 $((WORKSPACES - 1))); do
    SESSION_ID[$i]="$(echo "$SESSIONS_JSON" | jq -r --arg title "stress-$i" '.[] | select(.title == $title) | .id' | head -1)"
    [ -n "${SESSION_ID[$i]}" ] && [ "${SESSION_ID[$i]}" != "null" ] || fail "session $i missing"
done
echo "ok: $WORKSPACES sessions registered"

log "Streaming messages concurrently"
JOBS=(); for i in $(seq 0 $((WORKSPACES - 1))); do JOBS+=("send_message $i hello-w$i"); done
parallel "${JOBS[@]}" || fail "concurrent message send"
JOBS=(); for i in $(seq 0 $((WORKSPACES - 1))); do JOBS+=("wait_echo $i"); done
parallel "${JOBS[@]}" || fail "concurrent echo wait"
echo "ok: all echoes received"

log "Verifying session isolation"
for i in $(seq 0 $((WORKSPACES - 1))); do
    content="$(session_detail "${SESSION_ID[$i]}" | jq -r '[.messages[]?.content] | join("")')"
    for j in $(seq 0 $((WORKSPACES - 1))); do
        [ "$i" -eq "$j" ] && continue
        echo "$content" | grep -q "hello-w$j" && fail "session $i leaked session $j output"
    done
    echo "$content" | grep -q "Echo: hello-w$i" || fail "session $i missing its own echo"
done
echo "ok: no cross-session leakage"

log "Approving tool permissions concurrently"
JOBS=(); for i in $(seq 0 $((WORKSPACES - 1))); do JOBS+=("send_message $i /tool"); done
parallel "${JOBS[@]}" || fail "concurrent /tool send"
JOBS=(); for i in $(seq 0 $((WORKSPACES - 1))); do JOBS+=("approve_pending $i"); done
parallel "${JOBS[@]}" || fail "concurrent permission approval"
echo "ok: $WORKSPACES permissions approved"

log "Interrupting three long-running turns concurrently"
JOBS=(); for i in 0 1 2; do JOBS+=("send_message $i /slow"); done
parallel "${JOBS[@]}" || fail "concurrent /slow send"
sleep 0.4
JOBS=(); for i in 0 1 2; do JOBS+=("interrupt_session $i"); done
parallel "${JOBS[@]}" || fail "concurrent interrupt"
echo "ok: interrupts handled"

log "Multi-user organizations and roles"
OTHER_ORG="$(api -X POST -H 'Content-Type: application/json' -d '{"name":"other-org"}'     "http://127.0.0.1:$CP_PORT/api/v1/orgs" | jq -r .id)"
[ -n "$OTHER_ORG" ] && [ "$OTHER_ORG" != "null" ] || fail "organization creation"
DEFAULT_ORG="org_default"
create_user() { # org, name, role -> JSON
    api -X POST -H 'Content-Type: application/json' \
        -d "{\"org_id\":\"$1\",\"name\":\"$2\",\"role\":\"$3\"}" \
        "http://127.0.0.1:$CP_PORT/api/v1/users"
}
ADA="$(create_user "$DEFAULT_ORG" "Ada" member)"
[ "$(echo "$ADA" | jq -r .user.role)" = "member" ] || fail "member creation"
ADA_ID="$(echo "$ADA" | jq -r .user.id)"
ADA_TOKEN="$(echo "$ADA" | jq -r .token)"
ROOT_USER="$(create_user "$DEFAULT_ORG" "Root" admin)"
ROOT_TOKEN="$(echo "$ROOT_USER" | jq -r .token)"
VERA_TOKEN="$(create_user "$DEFAULT_ORG" "Vera" viewer | jq -r .token)"
OTHER_TOKEN="$(create_user "$OTHER_ORG" "Other" member | jq -r .token)"

user_api() { curl -fsS -H "Authorization: Bearer $1" "${@:2}"; }
[ "$(user_api "$ADA_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/machines" | jq length)" -eq "$MACHINES" ] \
    || fail "member cannot see default-org machines"
[ "$(user_api "$OTHER_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/machines" | jq length)" -eq 0 ] \
    || fail "other-org member sees default-org machines"

MOVED_MACHINE=2
code="$(curl -sS -o "$TMP/move.json" -w '%{http_code}' -X PUT \
    -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    -d "{\"org_id\":\"$OTHER_ORG\"}" \
    "http://127.0.0.1:$CP_PORT/api/v1/machines/${MACHINE_ID[$MOVED_MACHINE]}/org")"
[ "$code" = "200" ] || fail "machine org assignment returned $code"
[ "$(user_api "$ADA_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/machines" | jq length)" -eq $((MACHINES - 1)) ] \
    || fail "former org still sees the moved machine"
[ "$(user_api "$OTHER_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/machines" | jq length)" -eq 1 ] \
    || fail "new org cannot see the moved machine"
user_api "$OTHER_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/workspaces/${WS_ID[4]}" >/dev/null \
    || fail "new org cannot read its moved workspace"
code="$(curl -sS -o "$TMP/cross.json" -w '%{http_code}' \
    -H "Authorization: Bearer $ADA_TOKEN" \
    "http://127.0.0.1:$CP_PORT/api/v1/workspaces/${WS_ID[4]}")"
[ "$code" = "404" ] || fail "cross-org workspace returned $code, expected 404"

code="$(curl -sS -o "$TMP/viewer.json" -w '%{http_code}' -X POST \
    -H "Authorization: Bearer $VERA_TOKEN" -H 'Content-Type: application/json' \
    -d '{"paths":["owner.txt"]}' \
    "http://127.0.0.1:$CP_PORT/api/v1/workspaces/${WS_ID[0]}/git/stage")"
[ "$code" = "403" ] || fail "viewer write returned $code, expected 403"
code="$(curl -sS -o "$TMP/member-users.json" -w '%{http_code}' \
    -H "Authorization: Bearer $ADA_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/users")"
[ "$code" = "403" ] || fail "member listing users returned $code, expected 403"
user_api "$ROOT_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/users" \
    | jq -e 'map(.name) | index("Ada") != null' >/dev/null || fail "admin cannot list users"

ROTATED="$(user_api "$ROOT_TOKEN" -X POST \
    "http://127.0.0.1:$CP_PORT/api/v1/users/$ADA_ID/rotate-token")"
NEW_ADA_TOKEN="$(echo "$ROTATED" | jq -r .token)"
code="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $ADA_TOKEN" \
    "http://127.0.0.1:$CP_PORT/api/v1/machines")"
[ "$code" = "401" ] || fail "old user token returned $code, expected 401"
[ "$(user_api "$NEW_ADA_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/machines" | jq length)" -eq $((MACHINES - 1)) ] \
    || fail "rotated user token does not work"
user_api "$ROOT_TOKEN" "http://127.0.0.1:$CP_PORT/api/v1/audit?limit=100" \
    | jq -e '.entries | length > 0' >/dev/null || fail "org admin cannot read the audit trail"
echo "ok: organization scoping, roles, token rotation and audit"

log "Taking machine 1 offline"
OFFLINE_MACHINE=1
kill -INT "${DAEMON_PID[$OFFLINE_MACHINE]}"
wait_for "http://127.0.0.1:$CP_PORT/api/v1/machines" \
    "(map(select(.status == \"online\")) | length) == $((MACHINES - 1))" \
    "machine did not go offline" >/dev/null || fail "machine 1 still online"

code="$(status_of "http://127.0.0.1:$CP_PORT/api/v1/sessions" "$TMP/offline-session.json" \
    -X POST -H 'Content-Type: application/json' \
    -d "{\"machine_id\":\"${MACHINE_ID[$OFFLINE_MACHINE]}\",\"provider\":\"mock\",\"cwd\":\"/tmp\"}")"
[ "$code" = "503" ] || fail "session on an offline machine returned $code, expected 503"
jq -e '.code == "machine_offline"' "$TMP/offline-session.json" >/dev/null \
    || fail "offline error code missing"
echo "ok: offline machine rejects new sessions"

log "Other machines keep serving while machine 1 is down"
SURVIVOR_WS="${WS_ID[0]}"
SURVIVOR_SESSION="$(api -X POST -H 'Content-Type: application/json' \
    -d "{\"machine_id\":\"${MACHINE_ID[0]}\",\"provider\":\"mock\",\"workspace_id\":\"$SURVIVOR_WS\",\"title\":\"survivor\"}" \
    "http://127.0.0.1:$CP_PORT/api/v1/sessions" | jq -r .id)"
api -X POST -H 'Content-Type: application/json' -d '{"content":"offline-survivor"}' \
    "http://127.0.0.1:$CP_PORT/api/v1/sessions/$SURVIVOR_SESSION/messages" >/dev/null
wait_for "http://127.0.0.1:$CP_PORT/api/v1/sessions/$SURVIVOR_SESSION" \
    '[.messages[]?.content] | join("") | contains("Echo: offline-survivor")' \
    "surviving machine stopped serving" >/dev/null || fail "surviving machine failed"
status_of "http://127.0.0.1:$CP_PORT/api/v1/sessions/$SURVIVOR_SESSION" "$TMP/delete.json" -X DELETE >/dev/null
echo "ok: surviving machines unaffected"

log "Restarting machine 1 and verifying workspace recovery"
DAEMON_PID[$OFFLINE_MACHINE]=""
start_daemon "$OFFLINE_MACHINE"
wait_for "http://127.0.0.1:$CP_PORT/api/v1/machines" \
    "length == $MACHINES and (map(.status) | all(. == \"online\"))" \
    "all machines did not come back online" >/dev/null || { dump_logs; fail "machine 1 did not reconnect"; }

MACHINE_JSON="$(api "http://127.0.0.1:$CP_PORT/api/v1/machines/${MACHINE_ID[$OFFLINE_MACHINE]}")"
[ "$(echo "$MACHINE_JSON" | jq '.workspaces | length')" -eq "$WORKSPACES_PER_MACHINE" ] \
    || fail "machine 1 lost its workspaces after restart: $MACHINE_JSON"
for i in 2 3; do
    tree_check "$i" || fail "workspace $i is not readable after reconnect"
    file_check "$i" || fail "workspace $i returned the wrong owner after reconnect"
done
echo "ok: workspaces survived the daemon restart"

log "Rotating machine 0's key and reconnecting"
kill -INT "${DAEMON_PID[0]}"
wait "${DAEMON_PID[0]}" 2>/dev/null || true
DAEMON_PID[0]=""
ROTATE_OUT="$("$DAEMON_BIN" --config "$TMP/daemon0.toml" rotate-key)" \
    || fail "rotate-key failed"
echo "$ROTATE_OUT" | jq -e '.previous_fingerprint != .new_fingerprint' >/dev/null \
    || fail "rotate-key output missing fingerprints"
[ -f "$TMP/data0/machine.key.previous" ] || fail "previous key was not kept"
start_daemon 0
wait_for "http://127.0.0.1:$CP_PORT/api/v1/machines" \
    "length == $MACHINES and (map(.status) | all(. == \"online\"))" \
    "rotated machine did not reconnect" >/dev/null || { dump_logs; fail "rotation reconnect"; }
for _ in $(seq 1 50); do
    [ ! -f "$TMP/data0/machine.key.previous" ] && break
    sleep 0.1
done
[ ! -f "$TMP/data0/machine.key.previous" ] || fail "previous key was not removed after rotation"
[ "$(api "http://127.0.0.1:$CP_PORT/api/v1/machines" | jq length)" -eq "$MACHINES" ] \
    || fail "rotation created a second machine identity"
tree_check 0 || fail "workspace unreadable after key rotation"
echo "ok: key rotation kept the machine id and workspaces"

log "Final soak: 60 concurrent reads"
JOBS=()
for round in $(seq 1 10); do
    for i in $(seq 0 $((WORKSPACES - 1))); do
        JOBS+=("tree_check $i")
    done
done
parallel "${JOBS[@]}" || fail "post-restart soak reads failed"
echo "ok: 60 post-restart reads"

log "Cleaning up sessions and workspaces"
for i in $(seq 0 $((WORKSPACES - 1))); do
    status_of "http://127.0.0.1:$CP_PORT/api/v1/sessions/${SESSION_ID[$i]}" "$TMP/delete.json" -X DELETE >/dev/null || true
done
for i in $(seq 0 $((WORKSPACES - 1))); do
    status_of "http://127.0.0.1:$CP_PORT/api/v1/workspaces/${WS_ID[$i]}" "$TMP/delete.json" -X DELETE >/dev/null || true
done

printf '\n\033[1;32mMULTI-MACHINE STRESS PASSED\033[0m\n'
