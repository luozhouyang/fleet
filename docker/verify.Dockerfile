# BuildKit is used by modern Docker; no external Dockerfile frontend image is
# required, which matters on networks where Docker Hub is restricted.

ARG RUST_IMAGE=rust:1.97.1-bookworm
ARG NODE_IMAGE=node:22-bookworm-slim
#
# Reproducible Linux verification image.
# Contains the Rust toolchain, Node/pnpm and Playwright Chromium so the full
# test suite (including UI E2E) runs inside one fresh container.

FROM ${NODE_IMAGE} AS node

FROM ${RUST_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_TERM_COLOR=always \
    PNPM_HOME=/usr/local/pnpm \
    PATH=/usr/local/pnpm:$PATH

ARG APT_MIRROR=
RUN if [ -n "$APT_MIRROR" ]; then \
        sed -i "s|deb.debian.org|$APT_MIRROR|g" /etc/apt/sources.list.d/debian.sources; \
    fi \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        jq \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Reuse Node from the official image instead of adding a second package source.
ARG NPM_REGISTRY=
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -sf /usr/local/lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -sf /usr/local/lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
    && ln -sf /usr/local/lib/node_modules/corepack/dist/corepack.js /usr/local/bin/corepack \
    && if [ -n "$NPM_REGISTRY" ]; then export COREPACK_NPM_REGISTRY="$NPM_REGISTRY"; fi \
    && corepack enable \
    && corepack prepare pnpm@10.22.0 --activate

WORKDIR /workspace

# Install JS dependencies in their own layer.
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json apps/web/package.json
COPY apps/docs/package.json apps/docs/package.json
COPY packages/api-client/package.json packages/api-client/package.json
COPY packages/protocol-types/package.json packages/protocol-types/package.json
COPY packages/ui/package.json packages/ui/package.json
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    if [ -n "$NPM_REGISTRY" ]; then pnpm config set registry "$NPM_REGISTRY"; fi \
    && pnpm install --frozen-lockfile

COPY . .

ARG CARGO_REGISTRY=
RUN if [ -n "$CARGO_REGISTRY" ]; then \
        printf '[source.crates-io]\nreplace-with = "mirror"\n[source.mirror]\nregistry = "%s"\n' \
            "$CARGO_REGISTRY" > /usr/local/cargo/config.toml; \
    fi

# Chromium plus its system libraries for Playwright.
ARG PLAYWRIGHT_DOWNLOAD_HOST=
RUN if [ -n "$PLAYWRIGHT_DOWNLOAD_HOST" ]; then \
        export PLAYWRIGHT_DOWNLOAD_HOST; \
    fi \
    && pnpm --filter @agent-fleet/web exec playwright install --with-deps chromium

ENV CARGO_BUILD_JOBS=4

CMD ["bash", "scripts/test/docker_verify.sh", "--inside", "all"]
