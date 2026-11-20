#!/bin/sh
# Upgrade smoke test for Fleet release archives.
#
# Installs a previous Linux x86_64 release into a temporary prefix, serves a
# new archive over local HTTP, and runs the real `fleet upgrade` path. The
# test asserts that the binaries and web assets are replaced, that no backup
# files are left behind, and that a corrupted checksum is rejected.
#
#   scripts/test/upgrade-smoke.sh \
#     --old-archive old/fleet-linux-x86_64.tar.gz \
#     --old-sums old/SHA256SUMS \
#     --new-archive new/fleet-linux-x86_64.tar.gz \
#     --new-version v0.2.2
#
# It writes only inside a freshly created temporary directory and does not
# install or restart services. Requires `tar`, `curl`, and `python3`.

set -eu

ASSET_NAME="fleet-linux-x86_64.tar.gz"

log() {
  printf '==> %s\n' "$*"
}

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: upgrade-smoke.sh --old-archive PATH --new-archive PATH --new-version VERSION [options]

Install a previous Fleet release, then upgrade to a new archive over local
HTTP using the real `fleet upgrade` command.

Options:
  --old-archive PATH     Previous Linux x86_64 release archive
  --old-sums PATH        Optional SHA256SUMS for the previous archive
  --new-archive PATH     New Linux x86_64 release archive
  --new-version VERSION  Tag to upgrade to (for example v0.2.2)
  -h, --help             Show this help

The previous archive is installed with `fleet install --no-service`, so no
service definitions are written.
EOF
}

old_archive=""
old_sums=""
new_archive=""
new_version=""

while [ $# -gt 0 ]; do
  case "$1" in
    --old-archive)
      [ $# -ge 2 ] || fail "--old-archive requires a value"
      old_archive="$2"
      shift 2
      ;;
    --old-sums)
      [ $# -ge 2 ] || fail "--old-sums requires a value"
      old_sums="$2"
      shift 2
      ;;
    --new-archive)
      [ $# -ge 2 ] || fail "--new-archive requires a value"
      new_archive="$2"
      shift 2
      ;;
    --new-version)
      [ $# -ge 2 ] || fail "--new-version requires a value"
      new_version="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1 (try --help)"
      ;;
  esac
done

[ -n "$old_archive" ] || {
  usage >&2
  fail "--old-archive is required"
}
[ -n "$new_archive" ] || {
  usage >&2
  fail "--new-archive is required"
}
[ -n "$new_version" ] || {
  usage >&2
  fail "--new-version is required"
}
[ -f "$old_archive" ] || fail "old archive not found: $old_archive"
[ -f "$new_archive" ] || fail "new archive not found: $new_archive"
if [ -n "$old_sums" ] && [ ! -f "$old_sums" ]; then
  fail "old SHA256SUMS not found: $old_sums"
fi

sha256_line() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1"
  else
    shasum -a 256 "$1"
  fi
}

sha256_file() {
  sha256_line "$1" | awk '{print $1}'
}

version_matches() {
  case "$1" in
    *"$2"*) return 0 ;;
    *) return 1 ;;
  esac
}

work=$(mktemp -d "${TMPDIR:-/tmp}/fleet-upgrade-smoke.XXXXXX")
server_pid=""

cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

if [ -n "$old_sums" ]; then
  expected=$(grep -E "[[:space:]]${ASSET_NAME}\$" "$old_sums" | awk '{print $1}' | head -n 1)
  [ -n "$expected" ] || fail "$old_sums has no entry for $ASSET_NAME"
  actual=$(sha256_file "$old_archive")
  [ "$expected" = "$actual" ] \
    || fail "old archive checksum mismatch: expected $expected, got $actual"
  log "verified the previous archive against $old_sums"
fi

mkdir -p "$work/old"
tar -xzf "$old_archive" -C "$work/old" || fail "cannot extract $old_archive"
for name in fleet fleet-admin fleet-daemon; do
  [ -x "$work/old/bin/$name" ] || fail "old archive is missing bin/$name"
done

prefix="$work/prefix"
log "installing the previous release into $prefix"
if ! "$work/old/bin/fleet" install \
  --from-dir "$work/old" \
  --prefix "$prefix" \
  --no-service \
  --force \
  --yes >"$work/install.log" 2>&1; then
  cat "$work/install.log" >&2
  fail "failed to install the previous release"
fi
old_output=$("$prefix/bin/fleet" --version) || fail "the installed fleet did not report a version"
log "installed: $old_output"

# Mark the installed web tree so the upgrade must replace it.
mkdir -p "$prefix/share/fleet/web"
printf 'stale web asset\n' >"$prefix/share/fleet/web/upgrade-sentinel"

mkdir -p "$work/serve"
cp "$new_archive" "$work/serve/$ASSET_NAME"
(
  cd "$work/serve"
  sha256_line "$ASSET_NAME" >SHA256SUMS
)

port=$(python3 - <<'PY'
import socket

with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
) || fail "could not allocate a local port"
log "serving the new archive on 127.0.0.1:$port"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$work/serve" \
  >"$work/http.log" 2>&1 &
server_pid=$!

ready=0
attempt=0
while [ "$attempt" -lt 10 ]; do
  if curl -fsS "http://127.0.0.1:$port/SHA256SUMS" >/dev/null 2>&1; then
    ready=1
    break
  fi
  attempt=$((attempt + 1))
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  cat "$work/http.log" >&2
  fail "the local HTTP server did not become ready"
fi

log "upgrading to $new_version"
if ! FLEET_BASE_URL="http://127.0.0.1:$port" \
  "$prefix/bin/fleet" upgrade \
  --version "$new_version" \
  --force \
  --no-restart \
  --yes >"$work/upgrade.log" 2>&1; then
  cat "$work/upgrade.log" >&2
  fail "upgrade failed"
fi
cat "$work/upgrade.log"

new_number=${new_version#v}
for name in fleet fleet-admin fleet-daemon; do
  output=$("$prefix/bin/$name" --version) || fail "$name --version failed"
  version_matches "$output" "$new_number" \
    || fail "$name reports '$output', expected $new_number"
  log "$output"
done

[ ! -e "$prefix/share/fleet/web/upgrade-sentinel" ] \
  || fail "the upgrade did not replace the web assets"
if find "$prefix" -name '*.old-*' | grep -q .; then
  find "$prefix" -name '*.old-*' >&2
  fail "the upgrade left backup files behind"
fi
log "binaries and web assets were replaced"

# A corrupted checksum must fail closed and leave the installation untouched.
printf '%064d  %s\n' 0 "$ASSET_NAME" >"$work/serve/SHA256SUMS"
if FLEET_BASE_URL="http://127.0.0.1:$port" \
  "$prefix/bin/fleet" upgrade \
  --version "$new_version" \
  --force \
  --no-restart \
  --yes >"$work/bad-upgrade.log" 2>&1; then
  fail "the upgrade accepted a bad checksum"
fi
grep -q 'checksum mismatch' "$work/bad-upgrade.log" || {
  cat "$work/bad-upgrade.log" >&2
  fail "the bad checksum did not report a checksum mismatch"
}
for name in fleet fleet-admin fleet-daemon; do
  output=$("$prefix/bin/$name" --version) || fail "$name --version failed after the bad upgrade"
  version_matches "$output" "$new_number" \
    || fail "$name changed after a rejected upgrade: $output"
done
log "a corrupted checksum was rejected without changing the installation"

log "upgrade smoke test passed"
