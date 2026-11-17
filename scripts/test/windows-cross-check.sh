#!/bin/sh
# Cross-check the Windows MSVC build on Linux using cargo-xwin.
#
# This is a compile check, not a substitute for Windows runtime validation.
# Task Scheduler registration, stored credentials, and startup behavior still
# need a Windows host or the windows-latest CI job.
#
#   scripts/test/windows-cross-check.sh [--no-proxy] [--image IMAGE]
#
# The first run is slow: it pulls a Rust image, installs cargo-xwin and
# clang/lld/llvm, downloads the MSVC CRT/SDK, and compiles the workspace for
# x86_64-pc-windows-msvc. Named volumes keep later runs warm.

set -eu

repo_root=$(CDPATH= cd "$(dirname "$0")/../.." && pwd)
image="${FLEET_WINDOWS_CROSS_IMAGE:-rust:bookworm}"
no_proxy=0

usage() {
  cat <<'EOF'
Usage: windows-cross-check.sh [--no-proxy] [--image IMAGE]

  --no-proxy   Clear proxy environment variables inside the container. Use
               this when ~/.docker/config.json points at a host-only proxy
               (for example 127.0.0.1:7897) that the container cannot reach.
  --image      Rust image (default: rust:bookworm or FLEET_WINDOWS_CROSS_IMAGE).
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --no-proxy)
      no_proxy=1
      shift
      ;;
    --image)
      [ $# -ge 2 ] || { echo "error: --image requires a value" >&2; exit 1; }
      image=$2
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1 (try --help)" >&2
      exit 1
      ;;
  esac
done

command -v docker >/dev/null 2>&1 || {
  echo "error: docker is required" >&2
  exit 1
}
docker info >/dev/null 2>&1 || {
  echo "error: cannot reach the Docker daemon" >&2
  exit 1
}

set -- docker run --rm --name fleet-win-check \
  -v "$repo_root":/work:ro -w /work \
  -v fleet-win-target:/target \
  -v fleet-cargo-registry:/usr/local/cargo/registry \
  -v fleet-xwin-cache:/root/.cache/cargo-xwin \
  -e CARGO_TARGET_DIR=/target
if [ "$no_proxy" -eq 1 ]; then
  set -- "$@" \
    -e http_proxy= -e https_proxy= \
    -e HTTP_PROXY= -e HTTPS_PROXY= \
    -e ALL_PROXY= -e all_proxy= -e NO_PROXY='*'
fi
if [ -n "${FLEET_RUSTUP_DIST_SERVER:-}" ]; then
  set -- "$@" -e "RUSTUP_DIST_SERVER=$FLEET_RUSTUP_DIST_SERVER"
fi
set -- "$@" "$image" bash -c '
  set -euo pipefail
  export PATH=/usr/local/cargo/bin:$PATH
  apt-get update
  apt-get install -y --no-install-recommends clang lld llvm cmake ninja-build pkg-config
  cargo install cargo-xwin --locked
  rustup target add x86_64-pc-windows-msvc
  cargo xwin check --locked --target x86_64-pc-windows-msvc \
    -p fleet-cli -p fleet-admin -p fleet-daemon
'
exec "$@"
