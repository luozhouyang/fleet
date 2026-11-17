#!/bin/sh
# Install smoke test for Fleet release archives.
#
# Host mode (default): assemble a temporary dist/ directory (archive +
# SHA256SUMS) and run the same clean-container flow as CI. The container
# bootstraps curl, creates a non-root user, and runs the installer as that
# user with FLEET_BASE_URL=file:///dist.
#
#   scripts/test/install-smoke.sh --archive PATH [--installer PATH] [--image IMAGE]
#
# --in-container is an internal mode used by the workflow (and by this script
# itself): it installs prerequisites, creates the non-root user, and runs the
# installer and assertions. It expects the installer at /install.sh, the dist
# directory at /dist, and this script at /smoke.sh. Those defaults can be
# overridden with FLEET_SMOKE_INSTALLER, FLEET_SMOKE_DIST, and FLEET_SMOKE_USER.
#
# This script writes only inside a freshly created temporary directory.

set -eu

SMOKE_USER="${FLEET_SMOKE_USER:-smoke}"
SMOKE_HOME="${FLEET_SMOKE_HOME:-/home/smoke}"
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
Usage: install-smoke.sh --archive PATH [--installer PATH] [--image IMAGE]

Install a Fleet Linux x86_64 release archive in a clean container as a
non-root user and exercise the install flow.

Options:
  --archive PATH    Release archive. It is copied into the container as
                    fleet-linux-x86_64.tar.gz, which is the asset name the
                    installer derives for linux/x86_64.
  --installer PATH  install.sh to test (default: apps/docs/public/install.sh)
  --image IMAGE     Container image (default: debian:12-slim)
  -h, --help        Show this help

Requires a running Docker daemon. The container needs network access the
first time it installs curl/ca-certificates into the base image.
EOF
}

# ---------------------------------------------------------------------------
# Shared helpers

case "$0" in
  */*) self_dir=$(dirname "$0") ;;
  *) self_dir=$(dirname "$(command -v "$0")") ;;
esac
SELF=$(CDPATH= cd "$self_dir" && pwd)/$(basename "$0")
SCRIPT_DIR=$(CDPATH= cd "$self_dir" && pwd)
REPO_ROOT=$(CDPATH= cd "$SCRIPT_DIR/../.." && pwd)

archive=""
installer=""
image="debian:12-slim"
in_container=0

while [ $# -gt 0 ]; do
  case "$1" in
    --archive)
      [ $# -ge 2 ] || fail "--archive requires a value"
      archive="$2"
      shift 2
      ;;
    --installer)
      [ $# -ge 2 ] || fail "--installer requires a value"
      installer="$2"
      shift 2
      ;;
    --image)
      [ $# -ge 2 ] || fail "--image requires a value"
      image="$2"
      shift 2
      ;;
    --in-container)
      in_container=1
      shift
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

verify_archive() {
  listing=$(tar -tzf "$1") || fail "cannot read archive: $1"
  for name in bin/fleet bin/fleet-admin bin/fleet-daemon; do
    printf '%s\n' "$listing" | grep -Eq "^(\./)?$name$" \
      || fail "archive is missing $name"
  done
}

# ---------------------------------------------------------------------------
# Host mode

host_main() {
  [ -n "$archive" ] || {
    usage >&2
    fail "--archive is required"
  }
  [ -f "$archive" ] || fail "archive not found: $archive"
  [ -n "$installer" ] || installer="$REPO_ROOT/apps/docs/public/install.sh"
  [ -f "$installer" ] || fail "installer not found: $installer"

  command -v docker >/dev/null 2>&1 || fail "docker is required"
  docker info >/dev/null 2>&1 || fail "cannot reach the Docker daemon"
  verify_archive "$archive"

  tmp_base="${TMPDIR:-/tmp}"
  tmp=$(mktemp -d "${tmp_base%/}/fleet-install-smoke.XXXXXX")
  trap 'rm -rf "$tmp"' EXIT INT TERM HUP

  dist="$tmp/dist"
  mkdir -p "$dist"
  cp "$archive" "$dist/$ASSET_NAME"
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$dist" && sha256sum "$ASSET_NAME" > SHA256SUMS)
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$dist" && shasum -a 256 "$ASSET_NAME" > SHA256SUMS)
  else
    fail "sha256sum or shasum is required to generate SHA256SUMS"
  fi

  log "image:     $image"
  log "archive:   $archive"
  log "installer: $installer"
  log "dist:      $dist"
  log "the installer runs as non-root user '$SMOKE_USER' in the container"

  rc=0
  docker run --rm \
    --name "fleet-install-smoke-$$" \
    -v "$dist:/dist:ro" \
    -v "$installer:/install.sh:ro" \
    -v "$SELF:/smoke.sh:ro" \
    "$image" sh /smoke.sh --in-container || rc=$?

  [ "$rc" -eq 0 ] || log "container exited with status $rc"
  return "$rc"
}

# ---------------------------------------------------------------------------
# Container mode (root bootstrap, then re-exec as the non-root user)

bootstrap_packages() {
  if [ -f /etc/alpine-release ] && command -v apk >/dev/null 2>&1; then
    log "installing prerequisites with apk"
    apk add --no-cache ca-certificates coreutils curl tar
  elif command -v apt-get >/dev/null 2>&1; then
    log "installing prerequisites with apt-get"
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends ca-certificates coreutils curl tar
  elif command -v dnf >/dev/null 2>&1; then
    log "installing prerequisites with dnf"
    dnf install -y ca-certificates coreutils curl tar
  else
    fail "unsupported base image: no apk, apt-get, or dnf found"
  fi
}

create_user() {
  if [ -f /etc/alpine-release ] && command -v adduser >/dev/null 2>&1; then
    adduser -D -h "$SMOKE_HOME" -s /bin/sh "$SMOKE_USER"
  elif command -v useradd >/dev/null 2>&1; then
    useradd -m -d "$SMOKE_HOME" -s /bin/sh "$SMOKE_USER"
  elif command -v adduser >/dev/null 2>&1; then
    adduser --disabled-password --gecos "" --home "$SMOKE_HOME" "$SMOKE_USER"
  else
    fail "cannot create non-root user: no useradd or adduser"
  fi
}

container_main() {
  dist_dir="${FLEET_SMOKE_DIST:-/dist}"
  installer_path="${FLEET_SMOKE_INSTALLER:-/install.sh}"

  if [ "$(id -u)" -eq 0 ]; then
    bootstrap_packages
    if ! id "$SMOKE_USER" >/dev/null 2>&1; then
      log "creating non-root user '$SMOKE_USER'"
      create_user
    fi
    log "re-running the install flow as '$SMOKE_USER'"
    # Options are placed before the username so this works with both
    # util-linux su (Debian/Ubuntu/Fedora) and BusyBox su (Alpine). HOME and
    # the mount paths are passed explicitly so no root environment leaks in.
    exec su -s /bin/sh -c \
      "HOME='$SMOKE_HOME' FLEET_SMOKE_DIST='$dist_dir' FLEET_SMOKE_INSTALLER='$installer_path' sh '$SELF' --in-container" \
      "$SMOKE_USER"
  fi

  user_main
}

# ---------------------------------------------------------------------------
# Non-root install flow and assertions

run_full_install_flow() {
  log "verifying the server-profile file installation"
  [ -f "$prefix/share/fleet/install.json" ] || fail "installer did not write share/fleet/install.json"

  log "admin setup (no service manager, fallback supervisor, no web assets)"
  "$bin/fleet" admin setup --yes --start --no-install-service --no-web

  # Server profile semantics: all state lives under the invoking user's
  # ~/.fleet; containers have no systemd, so services use the supervisor.
  [ -d "$HOME/.fleet/admin" ] || fail "expected admin state under $HOME/.fleet"

  log "waiting for http://127.0.0.1:8787/api/v1/health"
  i=0
  healthy=0
  while [ "$i" -lt 60 ]; do
    if curl -fsS http://127.0.0.1:8787/api/v1/health >/dev/null 2>&1; then
      healthy=1
      break
    fi
    i=$((i + 1))
    sleep 1
  done
  [ "$healthy" -eq 1 ] || fail "admin health endpoint did not become ready"

  log "daemon setup and enroll (fallback supervisor)"
  "$bin/fleet" daemon setup --yes --start --no-install-service \
    --org org_default --server ws://127.0.0.1:8787/ws/machine

  log "fleet status"
  i=0
  status_ok=0
  while [ "$i" -lt 30 ]; do
    if "$bin/fleet" status; then
      status_ok=1
      break
    fi
    i=$((i + 1))
    sleep 1
  done
  [ "$status_ok" -eq 1 ] || fail "fleet status did not report healthy roles"

  log "uninstall"
  "$bin/fleet" uninstall --yes
  [ ! -e "$bin/fleet" ] || fail "uninstall left $bin/fleet in place"
}

user_main() {
  [ "$(id -u)" -ne 0 ] || fail "the install flow must run as a non-root user"
  [ -f "$installer_path" ] || fail "installer not found at $installer_path"
  [ -f "$dist_dir/$ASSET_NAME" ] || fail "archive not found at $dist_dir/$ASSET_NAME"
  [ -f "$dist_dir/SHA256SUMS" ] || fail "SHA256SUMS not found at $dist_dir/SHA256SUMS"

  HOME="$SMOKE_HOME"
  export HOME
  prefix="$HOME/.local"
  bin="$prefix/bin"

  export FLEET_BASE_URL="file://$dist_dir"
  export PATH="$bin:$PATH"

  log "user: $(id -un) (uid $(id -u)); HOME=$HOME"
  log "installing with FLEET_BASE_URL=$FLEET_BASE_URL and --prefix $prefix"
  # Containers have no systemd; exercise the file installation here. The
  # service-manager path is covered by the host smoke job.
  sh "$installer_path" --prefix "$prefix" --no-service

  [ -x "$bin/fleet" ] || fail "installer did not install $bin/fleet"
  [ -x "$bin/fleet-admin" ] || fail "installer did not install $bin/fleet-admin"
  [ -x "$bin/fleet-daemon" ] || fail "installer did not install $bin/fleet-daemon"

  version=$("$bin/fleet" --version) || fail "fleet --version failed"
  log "fleet --version: $version"
  "$bin/fleet-admin" --version >/dev/null || fail "fleet-admin --version failed"
  "$bin/fleet-daemon" --version >/dev/null || fail "fleet-daemon --version failed"

  # `fleet install --from-dir` is now the only installer entry point; the
  # bootstrap script above exercised it with --no-service.
  if "$bin/fleet" install --help 2>/dev/null | grep -q -e '--from-dir'; then
    run_full_install_flow
  else
    fail "this build has no 'fleet install --from-dir'"
  fi
}

if [ "$in_container" -eq 1 ]; then
  container_main
else
  host_main
fi
