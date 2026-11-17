#!/bin/sh
#
# Fleet watchdog.
#
# Installed root-owned and scheduled by launchd (system domain) or a systemd
# system timer. It probes the admin and daemon health endpoints and restarts a
# service only after two consecutive failures, with cooldown and rate limits.
#
# SECURITY INVARIANTS
#   * Every placeholder below is substituted at install time by the Fleet
#     installer. Nothing here is read from a user-controlled file at runtime.
#   * There is no eval, no sourcing, and no parsing of user configuration.
#   * No Fleet binary is ever executed.
#   * Restart commands are a fixed allow-list selected by @SERVICE_MANAGER@.
#   * PATH is pinned to root-owned system directories so a user-writable
#     directory can never supply "curl", "systemctl", "launchctl", or friends.
#
# Exit status: 0 when both checks completed, 1 when the watchdog refused to
# act (missing curl, unusable state directory, persistence failure), and 2 for
# invalid or unsubstituted configuration at startup.

PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
set -u
umask 077

ADMIN_HEALTH_URL='@ADMIN_HEALTH_URL@'
DAEMON_HEALTH_URL='@DAEMON_HEALTH_URL@'
SERVICE_MANAGER='@SERVICE_MANAGER@'
ADMIN_SERVICE_ID='@ADMIN_SERVICE_ID@'
DAEMON_SERVICE_ID='@DAEMON_SERVICE_ID@'
STATE_DIR='@STATE_DIR@'

PROBE_TIMEOUT=3
FAILURE_THRESHOLD=2
ATTEMPT_WINDOW=600
MAX_ATTEMPTS=5
HEALTHY_RESET=1800
COOLDOWN_FIRST=30
COOLDOWN_SECOND=60
COOLDOWN_MAX=300

WATCHDOG_LOG="$STATE_DIR/watchdog.log"

log() {
    _log_line="$(date -u '+%Y-%m-%dT%H:%M:%SZ') fleet-watchdog: $*"
    printf '%s\n' "$_log_line" >&2
    if [ -d "$STATE_DIR" ]; then
        printf '%s\n' "$_log_line" >>"$WATCHDOG_LOG" 2>/dev/null || true
    fi
}

fail_config() {
    log "$*"
    exit 2
}

# ---------------------------------------------------------------------------
# Startup validation. Unsubstituted placeholders fail closed: no service is
# touched until every value is concrete and plausible.
# ---------------------------------------------------------------------------

case "$SERVICE_MANAGER" in
    launchd|systemd) ;;
    *) fail_config "invalid service manager '$SERVICE_MANAGER' (expected launchd or systemd)" ;;
esac

for _url in "$ADMIN_HEALTH_URL" "$DAEMON_HEALTH_URL"; do
    case "$_url" in
        http://?*|https://?*) ;;
        *) fail_config "invalid health URL '$_url' (expected an absolute http or https URL)" ;;
    esac
done

for _service_id in "$ADMIN_SERVICE_ID" "$DAEMON_SERVICE_ID"; do
    case "$_service_id" in
        ''|*[!A-Za-z0-9_.@-]*) fail_config "invalid service identifier '$_service_id'" ;;
        -*) fail_config "invalid service identifier '$_service_id' (must not start with '-')" ;;
    esac
done

case "$STATE_DIR" in
    /*) ;;
    *) fail_config "state directory must be an absolute path" ;;
esac

_current_uid=$(id -u 2>/dev/null) || _current_uid=1
case "$_current_uid" in
    ''|*[!0-9]*) _current_uid=1 ;;
esac
if [ "$_current_uid" -ne 0 ]; then
    log "must run as root; refusing to manage system services"
    exit 1
fi

# Fail safe: without curl we cannot distinguish healthy from unhealthy, so we
# must not restart anything.
if ! command -v curl >/dev/null 2>&1; then
    log "curl not found in PATH; refusing to manage services"
    exit 1
fi

# Create the state directory when absent. umask 077 makes it root-only.
if [ ! -d "$STATE_DIR" ]; then
    if ! mkdir -p "$STATE_DIR" 2>/dev/null; then
        log "cannot create state directory '$STATE_DIR'; refusing to manage services"
        exit 1
    fi
fi

if ! (: >"$STATE_DIR/.watchdog-write-test.$$") 2>/dev/null; then
    log "state directory '$STATE_DIR' is not writable; refusing to manage services"
    exit 1
fi
rm -f "$STATE_DIR/.watchdog-write-test.$$"

# ---------------------------------------------------------------------------
# State helpers. Counters and timestamps are validated as unsigned integers,
# so a damaged state file can never turn into command input.
# ---------------------------------------------------------------------------

read_number() {
    _number=0
    if [ -f "$1" ]; then
        if IFS= read -r _number <"$1"; then
            case "$_number" in
                ''|*[!0-9]*) _number=0 ;;
            esac
        else
            _number=0
        fi
    fi
    printf '%s\n' "$_number"
}

write_number() {
    _tmp="$1.tmp.$$"
    if printf '%s\n' "$2" >"$_tmp" 2>/dev/null; then
        if mv -f "$_tmp" "$1" 2>/dev/null; then
            return 0
        fi
    fi
    rm -f "$_tmp" 2>/dev/null || true
    return 1
}

prune_attempts() {
    _attempts_path=$1
    _cutoff=$(($(date +%s) - ATTEMPT_WINDOW))
    _tmp="$_attempts_path.tmp.$$"
    if ! : >"$_tmp" 2>/dev/null; then
        return 1
    fi
    if [ -f "$_attempts_path" ]; then
        while IFS= read -r _line || [ -n "$_line" ]; do
            case "$_line" in
                ''|*[!0-9]*) continue ;;
            esac
            if [ "$_line" -gt "$_cutoff" ]; then
                if ! printf '%s\n' "$_line" >>"$_tmp" 2>/dev/null; then
                    rm -f "$_tmp"
                    return 1
                fi
            fi
        done <"$_attempts_path"
    fi
    mv -f "$_tmp" "$_attempts_path" 2>/dev/null
}

count_attempts() {
    _attempt_count=0
    if [ -f "$1" ]; then
        while IFS= read -r _line || [ -n "$_line" ]; do
            case "$_line" in
                ''|*[!0-9]*) continue ;;
            esac
            _attempt_count=$((_attempt_count + 1))
        done <"$1"
    fi
    printf '%s\n' "$_attempt_count"
}

append_attempt() {
    printf '%s\n' "$2" >>"$1" 2>/dev/null
}

# ---------------------------------------------------------------------------
# Fixed probes and restart commands. These are the only external commands that
# act on the host; they take install-time values only.
# ---------------------------------------------------------------------------

probe_url() {
    # -q: ignore .curlrc. --noproxy: never route a loopback probe via a proxy.
    curl -q --fail --silent --show-error --max-time "$PROBE_TIMEOUT" --noproxy '*' "$1" >/dev/null 2>&1
}

service_is_active() {
    case "$SERVICE_MANAGER" in
        systemd)
            systemctl is-active --quiet "$1" >/dev/null 2>&1
            ;;
        launchd)
            launchctl print "system/$1" 2>/dev/null | grep -q 'state = running'
            ;;
        *)
            return 1
            ;;
    esac
}

restart_service() {
    case "$SERVICE_MANAGER" in
        systemd)
            systemctl restart "$1"
            ;;
        launchd)
            launchctl kickstart -k "system/$1"
            ;;
        *)
            return 1
            ;;
    esac
}

cooldown_for() {
    if [ "$1" -le 1 ]; then
        printf '%s\n' "$COOLDOWN_FIRST"
    elif [ "$1" -eq 2 ]; then
        printf '%s\n' "$COOLDOWN_SECOND"
    else
        printf '%s\n' "$COOLDOWN_MAX"
    fi
}

# ---------------------------------------------------------------------------
# Per-service check.
# ---------------------------------------------------------------------------

check_service() {
    _label=$1
    _url=$2
    _service_id=$3
    _prefix="$STATE_DIR/$_label"
    _failures_file="$_prefix.failures"
    _restarts_file="$_prefix.restarts"
    _attempts_file="$_prefix.attempts"
    _next_file="$_prefix.next_restart"
    _healthy_file="$_prefix.healthy_since"
    _rate_limited_file="$_prefix.rate_limited"

    _now=$(date +%s)

    _probe_ok=0
    _active_ok=0
    if probe_url "$_url"; then
        _probe_ok=1
    fi
    if service_is_active "$_service_id"; then
        _active_ok=1
    fi

    if [ "$_probe_ok" -eq 1 ] && [ "$_active_ok" -eq 1 ]; then
        _failures=$(read_number "$_failures_file")
        if [ "$_failures" -gt 0 ]; then
            log "$_label is healthy again"
            if ! write_number "$_failures_file" 0; then
                log "cannot persist failure counter for $_label"
            fi
        fi

        if [ -f "$_rate_limited_file" ]; then
            rm -f "$_rate_limited_file"
        fi

        _healthy_since=$(read_number "$_healthy_file")
        if [ "$_healthy_since" -eq 0 ]; then
            write_number "$_healthy_file" "$_now" || true
        elif [ "$((_now - _healthy_since))" -ge "$HEALTHY_RESET" ]; then
            _restarts=$(read_number "$_restarts_file")
            if [ "$_restarts" -gt 0 ]; then
                log "$_label has been healthy for 30 minutes; resetting watchdog counters"
            fi
            write_number "$_restarts_file" 0 || true
            write_number "$_healthy_file" "$_now" || true
            rm -f "$_attempts_file" "$_next_file" "$_rate_limited_file"
        fi
        return 0
    fi

    # Unhealthy or inactive. A broken health streak starts over.
    rm -f "$_healthy_file"

    if [ "$_probe_ok" -eq 0 ] && [ "$_active_ok" -eq 0 ]; then
        _reason="health probe failed and service is inactive"
    elif [ "$_probe_ok" -eq 0 ]; then
        _reason="health probe failed"
    else
        _reason="service is inactive"
    fi

    _failures=$(read_number "$_failures_file")
    _failures=$((_failures + 1))
    if ! write_number "$_failures_file" "$_failures"; then
        log "cannot persist failure counter for $_label; not restarting"
        return 1
    fi

    if [ "$_failures" -eq 1 ]; then
        log "$_label: $_reason (failure 1 of $FAILURE_THRESHOLD before restart)"
        return 0
    fi

    if ! prune_attempts "$_attempts_file"; then
        log "cannot persist restart history for $_label; not restarting"
        return 1
    fi
    _attempts=$(count_attempts "$_attempts_file")

    if [ "$_attempts" -ge "$MAX_ATTEMPTS" ]; then
        if [ ! -f "$_rate_limited_file" ]; then
            log "$_label: $MAX_ATTEMPTS restarts within 10 minutes; suspending restarts"
            : >"$_rate_limited_file" 2>/dev/null || true
        fi
        return 0
    fi

    _next_restart=$(read_number "$_next_file")
    if [ "$_now" -lt "$_next_restart" ]; then
        if [ "$_failures" -eq "$FAILURE_THRESHOLD" ]; then
            log "$_label: restart cooldown active until epoch $_next_restart; waiting"
        fi
        return 0
    fi

    _restarts=$(read_number "$_restarts_file")
    _restarts=$((_restarts + 1))
    _cooldown=$(cooldown_for "$_restarts")

    # Persist the decision before acting. If we cannot pace ourselves, we must
    # not restart.
    if ! write_number "$_restarts_file" "$_restarts"; then
        log "cannot persist restart counter for $_label; not restarting"
        return 1
    fi
    if ! append_attempt "$_attempts_file" "$_now"; then
        log "cannot persist restart history for $_label; not restarting"
        return 1
    fi
    if ! write_number "$_next_file" "$((_now + _cooldown))"; then
        log "cannot persist cooldown for $_label; not restarting"
        return 1
    fi
    rm -f "$_rate_limited_file"

    log "$_label: $_reason; restarting via $SERVICE_MANAGER (attempt $_restarts, cooldown ${_cooldown}s)"
    if restart_service "$_service_id"; then
        log "$_label restart command succeeded"
    else
        log "$_label restart command failed"
    fi

    # Require two fresh consecutive failures before the next restart.
    write_number "$_failures_file" 0 || log "cannot reset failure counter for $_label"
    return 0
}

_status=0
if ! check_service admin "$ADMIN_HEALTH_URL" "$ADMIN_SERVICE_ID"; then
    _status=1
fi
if ! check_service daemon "$DAEMON_HEALTH_URL" "$DAEMON_SERVICE_ID"; then
    _status=1
fi

exit "$_status"
