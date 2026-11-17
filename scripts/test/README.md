# Install smoke test

Clean-container validation of the Fleet release archive + installer flow.

`install-smoke.sh` packages an existing Linux x86_64 release archive into a
temporary `dist/` directory (archive + `SHA256SUMS`), then runs the installer
as a **non-root** user in a throwaway container with
`FLEET_BASE_URL=file:///dist` and `--prefix "$HOME/.local"`.

The same script is mounted into the container by
`.github/workflows/install-smoke.yml` and drives the assertions there.

## Requirements

- Docker with a running daemon.
- `sh`, `tar`, and either `sha256sum` (Linux) or `shasum` (macOS) on the host.
- A Linux x86_64 release archive containing:

  ```
  bin/fleet
  bin/fleet-admin
  bin/fleet-daemon
  ```

- Network access inside the container so the bootstrap step can install
  `curl`, `ca-certificates`, `coreutils`, and `tar` with `apk`, `apt-get`, or
  `dnf`.

## Usage

```sh
scripts/test/install-smoke.sh --archive /path/to/fleet-linux-x86_64.tar.gz
```

Options:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--archive PATH` | required | Release archive; copied into `dist/` under the canonical asset name `fleet-linux-x86_64.tar.gz` |
| `--installer PATH` | `apps/docs/public/install.sh` | Installer under test |
| `--image IMAGE` | `debian:12-slim` | Container image |
| `-h`, `--help` | | Show usage |

Examples:

```sh
# Test a local build against Fedora.
scripts/test/install-smoke.sh \
  --archive target/dist/fleet-linux-x86_64.tar.gz \
  --image fedora:41

# Test an explicit installer copy.
scripts/test/install-smoke.sh \
  --archive /tmp/fleet-linux-x86_64.tar.gz \
  --installer apps/docs/public/install.sh \
  --image alpine:3.20
```

The script exits non-zero on the first failed assertion. Container output is
streamed directly, so no separate `docker logs` collection is needed; the
temporary directory is removed on exit, including on failure. All writes stay
inside that temporary directory.

### Building an archive locally

The archive must be a Linux x86_64 (musl) build. On Linux:

```sh
rustup target add x86_64-unknown-linux-musl
sudo apt-get install -y musl-tools
CC_x86_64_unknown_linux_musl=musl-gcc \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --target x86_64-unknown-linux-musl \
    -p fleet-admin -p fleet-daemon -p fleet-cli

mkdir -p /tmp/fleet-pkg/bin
cp target/x86_64-unknown-linux-musl/release/fleet \
   target/x86_64-unknown-linux-musl/release/fleet-admin \
   target/x86_64-unknown-linux-musl/release/fleet-daemon /tmp/fleet-pkg/bin/
tar -czf /tmp/fleet-linux-x86_64.tar.gz -C /tmp/fleet-pkg .
```

On macOS use a musl cross toolchain (for example `cross`). The CI
`package-linux` job does exactly this build on `ubuntu-latest`, so downloading
its `fleet-linux-x86_64-dist` artifact is usually easier.

## What the container exercises now

For every matrix entry the harness, running as root:

1. installs `curl`, `ca-certificates`, `coreutils`, and `tar` with the image's
   package manager;
2. creates the unprivileged user `smoke` (home `/home/smoke`);
3. re-executes itself as that user via `su -`.

Then, as `smoke`:

1. runs `sh /install.sh --prefix "$HOME/.local"` with
   `FLEET_BASE_URL=file:///dist`;
2. relies on the installer to verify the archive's SHA-256 against
   `SHA256SUMS`;
3. asserts `~/.local/bin/{fleet,fleet-admin,fleet-daemon}` exist and are
   executable;
4. runs `fleet --version`, `fleet-admin --version`, and
   `fleet-daemon --version`.

The archive deliberately omits `web/`, so the installer prints its
"archive has no web/ directory" warning. That warning is expected and does not
fail the test. Building the web UI here would add a pnpm install/build to
every run; see the TODO below.

## Matrix

| Name | Image | Notes |
| --- | --- | --- |
| `debian-12` | `debian:12-slim` | `apt-get` |
| `ubuntu-24.04` | `ubuntu:24.04` | `apt-get` |
| `fedora-41` | `fedora:41` | `dnf` |
| `alpine-3.20` | `alpine:3.20` | `apk` + explicit `coreutils` for `install(1)` |

Containers have no systemd. The current installer is invoked without
`--service`; the future `fleet install` path uses `--no-service` and the
fallback supervisor (`fleet admin setup --start --no-install-service`).

**TODO:** add a VM job (or a systemd-enabled container) to cover
`install.sh --service`, system service installation, unit files, and the
system-level profile where services run as the invoking user. That coverage
does not belong in this container matrix.

## The workflow

`.github/workflows/install-smoke.yml` runs on `workflow_dispatch` and on
`push`/`pull_request` limited to:

- `apps/docs/public/install.sh`
- `apps/fleet-cli/**`
- `crates/**`
- `Cargo.toml`
- `Cargo.lock`
- `scripts/test/**`
- `.github/workflows/install-smoke.yml`

Jobs:

- `package-linux` builds the musl release binaries, assembles
  `dist/fleet-linux-x86_64.tar.gz` with `bin/`, writes `SHA256SUMS`, and
  uploads the `fleet-linux-x86_64-dist` artifact.
- `smoke` downloads the artifact and runs the same container command as the
  local harness for each matrix entry. Both jobs are required gates.

Run it manually against a branch with:

```sh
gh workflow run install-smoke.yml --ref <branch>
```

### Server-profile flow

The harness runs the same server-profile path as a real install, without a
service manager because containers have none:

1. `install.sh --prefix "$HOME/.local" --no-service`
2. assert `share/fleet/install.json` and state under `$HOME/.fleet`
3. `fleet admin setup --yes --start --no-install-service --no-web`
4. poll `http://127.0.0.1:8787/api/v1/health`
5. `fleet daemon setup --yes --start --no-install-service ...`
6. `fleet status` (retried until both roles report healthy)
7. `fleet uninstall --yes`, then assert the binary is gone

The native service-manager path (systemd/launchd/Task Scheduler) is covered
separately on a real host. Keep the container job on `--no-service`; it
validates archive verification, prefix layout, first-run configuration,
supervision fallback, and uninstall.

The matrix is a required gate. When adding an image, install its prerequisites
with `apk`, `apt-get`, or `dnf` in `bootstrap_packages` and keep the non-root
user flow unchanged.

## Windows cross-check (Docker)

`windows-cross-check.sh` compiles the Windows MSVC target on a Linux container
with `cargo-xwin`:

```sh
scripts/test/windows-cross-check.sh --no-proxy
```

`--no-proxy` clears Docker's injected proxy variables; use it when
`~/.docker/config.json` points at a host-only proxy such as `127.0.0.1:7897`.
The first run pulls a Rust image, installs `cargo-xwin`/clang/lld/llvm, downloads
the MSVC CRT/SDK, and compiles `fleet-cli`, `fleet-admin`, and `fleet-daemon`.
Named volumes (`fleet-win-target`, `fleet-cargo-registry`, `fleet-xwin-cache`)
keep later runs warm.

This is a **compile** check. Docker Desktop on macOS cannot run Windows
containers, so Task Scheduler registration, stored credentials, UAC elevation,
and startup behavior need a Windows host, a Windows VM, or the `windows-latest`
CI job in `.github/workflows/ci.yml`.

## Assumptions

- The archive is named/renamed to `fleet-linux-x86_64.tar.gz` because that is
  the asset name `install.sh` derives for `linux` + `x86_64`.
- The admin listens on the default `127.0.0.1:8787` and
  `/api/v1/health` is unauthenticated (it is mounted outside the auth
  middleware today).
- `fleet install --from-dir` accepts `--prefix` and `--no-service`; the
  installer continues to honor `FLEET_BASE_URL` with `file://` URLs.
- Base images can reach their package mirrors during the run.
