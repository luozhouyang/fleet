#!/usr/bin/env bash
# Lightweight installer check. Uses --dry-run/--root so it needs no root and
# no prebuilt binaries, and verifies that generated systemd units reflect the
# requested prefix and service users.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
INSTALL="$ROOT/scripts/install/install.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_contains() { # file expected
  grep -Fq -- "$2" "$1" || fail "expected '$2' in $1"
}

assert_absent() { # file pattern
  if grep -Eq -- "$2" "$1"; then
    fail "did not expect '$2' in $1"
  fi
}

run_dry() { # output-file args...
  local out="$1"
  shift
  env -u SUDO_USER bash "$INSTALL" --dry-run "$@" >"$out"
}

echo "== explicit prefix and users"
run_dry "$TMP/explicit" \
  --prefix /opt/fleet \
  --admin-user adminx \
  --daemon-user daemonx

assert_contains "$TMP/explicit" "User=adminx"
assert_contains "$TMP/explicit" "Group=adminx"
assert_contains "$TMP/explicit" "User=daemonx"
assert_contains "$TMP/explicit" "Group=daemonx"
assert_contains "$TMP/explicit" "ExecStart=/opt/fleet/bin/agent-admin --config /etc/fleet/admin.toml serve --web-dir /opt/fleet/share/fleet/web"
assert_contains "$TMP/explicit" "ExecStart=/opt/fleet/bin/fleet-daemon --config /etc/fleet/daemon.toml serve"
assert_contains "$TMP/explicit" "EnvironmentFile=-/etc/fleet/admin.env"
assert_contains "$TMP/explicit" "EnvironmentFile=-/etc/fleet/daemon.env"
assert_contains "$TMP/explicit" "ReadWritePaths=/var/lib/fleet-admin -/var/lib/fleet"
assert_absent "$TMP/explicit" "/usr/local"
assert_absent "$TMP/explicit" "@[A-Z_]+@"

echo "== defaults without sudo"
run_dry "$TMP/defaults"
assert_contains "$TMP/defaults" "ExecStart=/usr/local/bin/agent-admin"
[ "$(grep -c '^User=fleet$' "$TMP/defaults")" -eq 2 ] \
  || fail "both services should default to User=fleet"
assert_absent "$TMP/defaults" "@[A-Z_]+@"

echo "== legacy --user shorthand"
run_dry "$TMP/shared" --user shared
[ "$(grep -c '^User=shared$' "$TMP/shared")" -eq 2 ] \
  || fail "--user should set both service users"

echo "== staged root"
STAGE="$TMP/stage"
run_dry "$TMP/rooted" \
  --root "$STAGE" \
  --prefix /opt/fleet \
  --admin-user adminx \
  --daemon-user daemonx
assert_contains "$TMP/rooted" "ExecStart=$STAGE/opt/fleet/bin/agent-admin"
assert_contains "$TMP/rooted" "EnvironmentFile=-$STAGE/etc/fleet/admin.env"
assert_contains "$TMP/rooted" "ReadWritePaths=$STAGE/var/lib/fleet-admin -$STAGE/var/lib/fleet"
[ ! -e "$STAGE" ] || fail "--dry-run must not create the staged root"

echo "installer check passed"
