#!/usr/bin/env bash
# Reproducible Linux verification in a fresh container.
#
#   ./scripts/test/docker_verify.sh quick   # fmt, clippy, Rust + web checks
#   ./scripts/test/docker_verify.sh e2e     # quick + API/Git/multi-machine E2E
#   ./scripts/test/docker_verify.sh all     # e2e + Playwright UI E2E
#   ./scripts/test/docker_verify.sh fresh   # all, without host cargo/target caches
#
# The image contains Rust, Node/pnpm and Playwright Chromium. The E2E scripts
# bind 127.0.0.1 inside the container, so no ports are published.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE="${DOCKER_TEST_IMAGE:-agent-fleet-verify}"
MODE="${1:-all}"

if [ "${1:-}" = "--inside" ]; then
    MODE="${2:-all}"
    export PCARGO="${PCARGO:-cargo}"
    export CI=1
    export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
    cd /workspace

    echo "== cargo fmt"
    "$PCARGO" fmt --all -- --check
    echo "== clippy"
    "$PCARGO" clippy --workspace --all-targets -- -D warnings
    echo "== Rust tests"
    "$PCARGO" test --workspace
    echo "== web typecheck + lint"
    pnpm --filter @agent-fleet/web exec tsc -p tsconfig.app.json --noEmit
    pnpm --filter @agent-fleet/web lint
    echo "== web build"
    pnpm --filter @agent-fleet/web build

    echo "== installer dry-run"
    ./scripts/test/install_check.sh

    if [ "$MODE" = "quick" ]; then
        echo "QUICK VERIFICATION PASSED"
        exit 0
    fi

    echo "== API smoke"
    ./scripts/test/e2e_smoke.sh
    echo "== Git write E2E"
    ./scripts/test/git_write_e2e.sh
    echo "== multi-machine stress"
    ./scripts/test/multi_machine_stress.sh

    if [ "$MODE" = "e2e" ]; then
        echo "E2E VERIFICATION PASSED"
        exit 0
    fi

    echo "== Playwright UI E2E"
    ./scripts/test/ui_e2e.sh
    echo "FULL VERIFICATION PASSED"
    exit 0
fi

case "$MODE" in
    quick | e2e | all | fresh) ;;
    *)
        echo "usage: $0 [quick|e2e|all|fresh]" >&2
        exit 2
        ;;
esac

DOCKER_ARGS=(--rm --init --shm-size=1g)
VOLUME_ARGS=()
BUILD_ARGS=(--pull=false)
for name in RUST_IMAGE NODE_IMAGE APT_MIRROR NPM_REGISTRY CARGO_REGISTRY PLAYWRIGHT_DOWNLOAD_HOST; do
    if [ -n "${!name:-}" ]; then
        BUILD_ARGS+=(--build-arg "$name=${!name}")
    fi
done
if [ "$MODE" = "fresh" ]; then
    MODE=all
    BUILD_ARGS+=(--no-cache)
else
    docker volume create agent-fleet-cargo-registry >/dev/null
    docker volume create agent-fleet-target >/dev/null
    VOLUME_ARGS=(
        -v agent-fleet-cargo-registry:/usr/local/cargo/registry
        -v agent-fleet-target:/workspace/target
    )
fi

echo "Building $IMAGE (context: $ROOT)"
docker build "${BUILD_ARGS[@]}" -f "$ROOT/docker/verify.Dockerfile" -t "$IMAGE" "$ROOT"

echo "Running mode: $MODE"
docker run "${DOCKER_ARGS[@]}" "${VOLUME_ARGS[@]}" \
    -e VERIFY_MODE="$MODE" \
    "$IMAGE" \
    bash scripts/test/docker_verify.sh --inside "$MODE"
