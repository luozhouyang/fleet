# Pairing Codes and Per-Machine Tokens

- **Status:** Draft for review
- **Date:** 2026-11-16
- **Scope:** admin ↔ daemon machine authentication, same-host / same-LAN onboarding only
- **Replaces:** the shared `AGENT_FLEET_MACHINE_TOKEN` model entirely
- **Backward compatibility:** not required; the product is not launched yet
- **Reference design:** `docs/connectivity.md` §2

## 1. Summary

Replace the shared machine token with a two-step model:

1. An organization administrator creates a short-lived, single-use **pairing code**.
2. The daemon presents the code during its first WebSocket handshake, proves
   possession of its Ed25519 key, and receives a **per-machine long-lived
   token**.
3. Every later connection authenticates with that per-machine token *and* the
   existing Ed25519 challenge/response.

The pairing code only bootstraps trust. It never replaces possession of the
machine key, and it cannot be replayed.

This design follows `docs/connectivity.md` §2. It adds the implementation
details needed to make that design safe.

The product has not been released. This iteration removes the shared machine
token and the trust-on-first-use (TOFU) path instead of keeping them as
compatibility fallbacks. Every machine must be paired before it can connect.

## 2. Context

Current implementation:

- `crates/core-connection/src/server.rs:553` authenticates the `/ws/machine`
  upgrade with a shared `TokenAuth` bearer token.
- `crates/core-connection/src/server.rs:135` performs
  `hello → challenge → auth → accepted` and pins the first public key seen
  (trust on first use).
- `crates/core-connection/src/client.rs:205` sends the shared token in the
  upgrade `Authorization` header.
- `apps/fleet-daemon/src/app.rs:59` only connects when both `server_url` and
  `connection.token` are configured.
- `apps/fleet-admin/src/main.rs:105` refuses to start without a non-empty
  `auth.machine_token` that differs from the admin token.
- `apps/fleet-cli/src/network.rs:524` writes the shared token into the daemon
  TOML during the network wizard.
- `scripts/install/install.sh:371` generates and distributes one shared token
  to both `admin.env` and `daemon.env`.

Problems with the current model:

- one leaked global token is an entry point for every admin;
- every new machine needs the shared token distributed out of band;
- a token leak during first contact lets an attacker pin their own key;
- there is no record of which user enrolled which machine into which
  organization;
- revocation exists per machine, but the credential itself remains shared.

## 3. Goals

- A per-machine credential that can be revoked without affecting other
  machines.
- An auditable pairing record: code creator, organization, machine id,
  public key fingerprint and timestamp.
- Same-host / same-LAN one-command setup:
  `af setup --token $AGENT_FLEET_ADMIN_TOKEN`.
- Remove the shared machine token and TOFU entirely; every machine must be
  paired.
- Clear, documented limits: pairing requires the daemon host to reach the
  admin; remote and offline enrollment are out of scope.

## 4. Non-goals

This iteration does **not** implement:

- backward compatibility with the shared-token or TOFU model; the product is
  not launched and the old paths are removed;
- remote machines that cannot reach the admin, or offline / deferred
  enrollment; only same-host / same-LAN setup is supported;
- remote code handoff with a polished UX (no QR code or guided page);
- a browser pairing approval screen or OAuth device flow;
- per-IP throttling that is accurate behind an untrusted reverse proxy;
- per-machine token expiry, independent token-rotation API, or multiple
  tokens per machine;
- mTLS enrollment;
- multiple admin instances / connection leases.

These are listed in §19 with the recommended order of work.

## 5. Terminology

| Term | Meaning |
|---|---|
| **pairing code** | Short-lived, single-use secret that authorizes the first key pin. Stored only as a hash. |
| **per-machine token** | Long-lived random bearer token bound to one machine id. Stored only as a hash. |
| **paired machine** | A machine with a stored per-machine token hash. Every connection must present its token. |
| **pairing mode** | A WebSocket connection whose `connection.hello` carries a `pairing_code`. |

## 6. End-to-end flows

### 6.1 Same-host `af setup`

```text
operator on the daemon host
  af setup --url http://127.0.0.1:8787 --token $AGENT_FLEET_ADMIN_TOKEN --org org_default

  1. CLI loads the daemon config, resolves machine.key_file
  2. CLI loads or creates:
       <key_file>        Ed25519 key pair (0600)
       <key_file>.id     stable machine id (0600)
  3. CLI probes GET /api/v1/health; unreachable => fail with instructions
  4. CLI calls POST /api/v1/pairing/codes with the admin token
       -> { id, code, expires_at, org_id }
  5. CLI opens ws://.../ws/machine without Authorization
       hello { machine_id, public_key, pairing_code }
       <- challenge
       -> auth { signature over nonce }
       <- accepted { machine_token: "afm_..." }
  6. CLI writes the token to <key_file>.token (0600)
  7. CLI edits the daemon TOML:
       [connection] server_url = "ws://127.0.0.1:8787/ws/machine"
       [machine]    id = "m_..."
       removes any legacy [connection] token and [connection] pairing_code keys
  8. operator starts the daemon
  9. daemon connects with Authorization: Bearer afm_...
```

The pairing code is generated and consumed inside the command and is not
printed, so it does not enter shell history or terminal scrollback.

### 6.2 Documented daemon command

```bash
fleet-daemon pair \
  --server ws://127.0.0.1:8787/ws/machine \
  --code 7Q2M-9XKR-4TWD
```

Same handshake as §6.1, but the token is only saved; configuration is not
rewritten unless `--write-config` is passed.

### 6.3 Subsequent connection

```text
daemon
  -> WS upgrade with Authorization: Bearer afm_...
  -> hello { machine_id, public_key, no pairing_code }
  <- challenge
  -> auth { signature }
  <- accepted
```

Server checks:

1. token hash resolves to `machine_id`;
2. `hello.machine_id` equals the token's machine id;
3. the machine has a pinned key and is not revoked;
4. the presented public key matches the pinned key, or a rotation proof signed
   by the old key is present;
5. the challenge signature verifies.

### 6.4 Re-pairing (token rotation / recovery)

An administrator creates a new code. The daemon performs the pairing handshake
again with the same machine id:

- same public key: allowed; the per-machine token hash is replaced and a new
  token is returned. The old token stops working immediately.
- different public key: allowed only with a `KeyRotationProof` signed by the
  previously pinned key. A pairing code does not bypass key ownership.

If the daemon crashed after the code was consumed but before the token was
persisted, the machine row exists without a usable token. Recovery is to create
a new code and pair again with the same key.

### 6.5 Revocation

`DELETE /api/v1/machines/{id}` keeps its current semantics:

1. write a permanent `machine_revocations` row;
2. disconnect the live connection;
3. delete the machine row, which also deletes its token hash.

A revoked machine cannot pair again until an administrator unrevokes it.
Because the machine row is gone, its old token is invalid.

### 6.6 No legacy path

The product has not been launched, so there is no legacy machine state. A
machine without a per-machine token cannot connect. The shared machine token,
trust-on-first-use pinning, and the key-only compatibility path are removed.

## 7. Protocol changes

### 7.1 `HelloPayload`

`crates/core-protocol/src/connection/auth.rs`:

```rust
pub struct HelloPayload {
    pub machine_id: MachineId,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub public_key: String,
    /// Present only on a pairing connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_code: Option<String>,
}
```

### 7.2 `ConnectionAccepted`

```rust
pub struct ConnectionAccepted {
    pub connection_id: String,
    pub machine_id: MachineId,
    pub server_version: String,
    pub protocol_version: u16,
    pub heartbeat_interval_ms: u64,
    /// Returned exactly once on successful pairing. Only its hash is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_token: Option<String>,
}
```

### 7.3 Secret redaction

`HelloPayload` and `ConnectionAccepted` must implement `Debug` manually:

```rust
impl fmt::Debug for HelloPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HelloPayload")
            // ...
            .field("pairing_code", &self.pairing_code.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}
```

`ConnectionAccepted` does the same for `machine_token`. Remove `Debug` from the
`#[derive(...)]` list on both types; deriving it is forbidden. No code path may
log the full struct at `debug`/`trace` level.

### 7.4 Error codes

`crates/core-common/src/lib.rs`:

| Constant | Value | Meaning |
|---|---|---|
| `PAIRING_CODE_INVALID` | `pairing_code_invalid` | code missing, unknown, expired, used, locked, or signature rejected |
| `MACHINE_NOT_PAIRED` | `machine_not_paired` | unpinned machine without a code |
| `MACHINE_TOKEN_REQUIRED` | `machine_token_required` | paired machine connected without its per-machine token |
| `MACHINE_KEY_MISMATCH` | `machine_key_mismatch` | re-pairing with a different key and no rotation proof |
| `MACHINE_ORG_MISMATCH` | `machine_org_mismatch` | machine already belongs to another organization |

`crates/core-connection/src/error.rs` adds terminal client variants:

```rust
NotPaired(String),
TokenRequired(String),
PairingCodeInvalid(String),
```

`apps/fleet-daemon/src/app.rs` treats all three like `Revoked`: log a clear
actionable message and stop the reconnect loop.

### 7.5 Pairing code REST types

`crates/core-protocol/src/pairing.rs`:

```rust
pub struct CreatePairingCodeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
}

pub struct PairingCodeResponse {
    pub id: String,
    pub code: String,
    pub expires_at: Timestamp,
    pub org_id: String,
}
```

`PairingCodeResponse` implements `Debug` with `code: "<redacted>"`.

## 8. Data model

Migration `crates/admin-storage/migrations/0003_pairing.sql`:

```sql
CREATE TABLE IF NOT EXISTS pairing_codes (
    id                 TEXT PRIMARY KEY,
    code_hash          TEXT NOT NULL UNIQUE,
    org_id             TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    created_by         TEXT NOT NULL,
    machine_name_hint  TEXT,
    expires_at         TEXT NOT NULL,
    used_at            TEXT,
    used_by_machine_id TEXT,
    attempts           INTEGER NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pairing_codes_org ON pairing_codes(org_id);
CREATE INDEX IF NOT EXISTS idx_pairing_codes_expires ON pairing_codes(expires_at);

ALTER TABLE machines ADD COLUMN token_hash TEXT;
ALTER TABLE machines ADD COLUMN token_created_at TEXT;
ALTER TABLE machines ADD COLUMN paired_at TEXT;
ALTER TABLE machines ADD COLUMN paired_by TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_machines_token_hash
    ON machines(token_hash)
    WHERE token_hash IS NOT NULL;
```

Notes:

- `paired_at` and `paired_by` are audit metadata; no connection logic depends
  on them.
- One token per machine is sufficient. Token rotation replaces `token_hash`.
- Deleting a machine deletes its token hash. `machine_revocations` prevents
  silent re-pairing.
- No plaintext code or token is stored.

## 9. Storage operations

### 9.1 `pairing_codes` repository

`crates/admin-storage/src/pairing_codes.rs`:

```rust
pub enum PairingCodeStatus {
    Valid(PairingCodeInfo),
    Invalid,
    Expired { id: String, org_id: String },
    Used { id: String, org_id: String },
    Locked { id: String, org_id: String },
}

pub struct PairingCodeInfo {
    pub id: String,
    pub org_id: String,
    pub created_by: String,
    pub machine_name_hint: Option<String>,
    pub expires_at: Timestamp,
    pub attempts_left: u32,
}
```

Methods:

- `create(org_id, created_by, machine_name_hint, ttl, code_hash) -> id`
- `lookup(code_hash) -> PairingCodeStatus`
- `record_failure(id) -> attempts`
- `consume(id, machine_id) -> Option<ConsumedCode>`:
  inside the transaction, select the code row, verify `used_at IS NULL` and
  `expires_at > now`, then
  `UPDATE pairing_codes SET used_at = ?, used_by_machine_id = ?
   WHERE id = ? AND used_at IS NULL`;
  returns the row's organization and creator on success.

```rust
pub struct ConsumedCode {
    pub org_id: String,
    pub created_by: String,
    pub machine_name_hint: Option<String>,
}
```

`lookup` returns distinct internal statuses for auditing, but the WebSocket
error is always `pairing_code_invalid` so clients cannot enumerate codes.

### 9.2 Machine pairing

`crates/admin-storage/src/machines.rs`:

- `machine_state(machine_id) -> { pinned_key: Option<String> }`
- `by_token_hash(token_hash) -> Option<String>`
- `complete_pairing(request: PairingCompletion) -> PairingOutcome`

`PairingCompletion`:

```rust
pub struct PairingCompletion {
    pub code_id: String,
    pub machine_id: String,
    pub public_key: String,
    pub token_hash: String,
    pub machine_name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub rotation_verified: bool,
}

pub struct PairingOutcome {
    pub machine_id: String,
    pub org_id: String,
    pub paired_at: Timestamp,
}
```

The organization, creator and optional machine-name hint are read from the
pairing-code row inside the transaction; the caller cannot supply them.

`complete_pairing` runs in one SQLite transaction:

1. `consume(code_id, machine_id) -> Option<ConsumedCode>`; `None` =>
   `PairingCodeInvalid`.
2. Reject if `machine_revocations` contains `machine_id`.
3. Load the machine row.
4. If it exists and `public_key` differs and `rotation_verified` is false =>
   `MachineKeyMismatch`.
5. If `machine_orgs` contains a different organization than
   `ConsumedCode.org_id` => `MachineOrgMismatch`.
6. Insert or update the machine:
   `name = ConsumedCode.machine_name_hint.unwrap_or(machine_name)`,
   `public_key`, `protocol_version`, `token_hash`, `token_created_at`,
   `paired_at`, `paired_by = ConsumedCode.created_by`, and the supplied
   metadata. Existing metadata fields that the caller does not supply keep
   their current values.
7. Upsert `machine_orgs(machine_id, ConsumedCode.org_id)`.
8. Append `machine.paired` audit:
   actor `ConsumedCode.created_by`, org `ConsumedCode.org_id`, subject
   `machine_id`, details `{ code_id, public_key_fingerprint, outcome: "ok" }`.
9. Commit and return `{ machine_id, org_id, paired_at }`.

The gateway verifies the Ed25519 signature and rotation proof before calling
this method. Storage never performs cryptography.

### 9.3 Token lookup

`by_token_hash` uses the unique partial index. Lookup failure is
indistinguishable from an unknown token and returns `None`.

## 10. Admin HTTP API

### 10.1 Create pairing code

`POST /api/v1/pairing/codes` in the protected router.

Authorization: an administrator bearer token (`acl::ensure_admin`).

Request:

```json
{ "org_id": "org_default", "machine_name": "workstation-01" }
```

Organization resolution:

- the super-admin REST token: `request.org_id` or `org_default`;
- user admin: `principal.org_id`; a different `request.org_id` is 403;
- the organization must exist (404 otherwise).

Response:

```json
{
  "id": "pair_...",
  "code": "7Q2M-9XKR-4TWD",
  "expires_at": "2026-11-16T00:10:00Z",
  "org_id": "org_default"
}
```

The plaintext code is returned once. Audit `pairing.code.create` records
`code_id`, `org_id`, `expires_at`, `machine_name_hint`; it never records the
code.

### 10.2 No REST redeem endpoint

Pairing is completed by the WebSocket handshake described in §11. A separate
redeem endpoint would duplicate the challenge flow and weaken the rule that the
code never replaces possession-of-key.

## 11. Gateway authentication state machine

`apps/fleet-admin/src/state.rs:46` builds the gateway.
`crates/core-connection/src/server.rs:553` resolves upgrade auth and
`crates/core-connection/src/server.rs:135` runs the handshake.

### 11.1 Upgrade authentication

```rust
enum UpgradeAuth {
    MachineToken { machine_id: String },
    Pairing,
}
```

Resolution:

1. No `Authorization` header => `Pairing`; the handshake must carry a pairing
   code.
2. Parse `Authorization: Bearer <credential>`; a malformed header => 401.
3. `hash_token(credential)` lookup in `machines.token_hash`:
   - hit => `MachineToken { machine_id }`;
   - miss => 401.

There is no shared-token fallback and no unauthenticated key-only path. Every
non-pairing connection must present a valid per-machine token.

### 11.2 Handshake

1. Read `connection.hello`; validate protocol version, non-empty
   `machine_id` / `public_key`, and `pairing_code` length (max 32 normalized
   characters).
2. Reject revoked machines with `FORBIDDEN` (client terminates).
3. Load `machine_state(machine_id)` => `{ pinned_key }`.
4. Select mode:

   **Pairing mode** when `hello.pairing_code` is present:
   - if `UpgradeAuth::MachineToken` is also present, require the token's
     machine id to equal `hello.machine_id`;
   - `pairing_codes.lookup(hash(normalized code))`:
     - `Invalid` / `Expired` / `Used` / `Locked` =>
       `pairing_code_invalid`;
   - continue. A missing `pinned_key` is authorized by the code; an existing
     key must match or be rotated with a proof.

   **Token mode** when `UpgradeAuth::MachineToken { machine_id }`:
   - require `machine_id == hello.machine_id`;
   - require `pinned_key.is_some()`, otherwise `machine_not_paired`;
   - no pairing code is issued.

   **No credential** otherwise (`UpgradeAuth::Pairing` without a code):
   - `pinned_key.is_some()` => `machine_token_required`;
   - `pinned_key.is_none()` => `machine_not_paired`.

5. Send `connection.auth_challenge`.
6. Read `connection.auth`; require matching `challenge_id` and a valid Ed25519
   signature over the nonce.
7. On a signature failure in pairing mode, call
   `pairing_codes.record_failure(code_id)`, append a `machine.pair_denied`
   audit entry for the known code, and return `pairing_code_invalid`.
8. If `pinned_key` exists and differs from `hello.public_key`, verify the
   `KeyRotationProof` signed by the pinned key. This applies in pairing and
   token modes.
9. If pairing mode:
   - generate `machine_token = afm_<64 hex>`;
   - call `complete_pairing(...)` with `token_hash = hash_token(machine_token)`
     and `rotation_verified`;
   - set `ConnectionAccepted.machine_token = Some(machine_token)`.
10. If rotation and not pairing mode: call `pin_key` with the new key. In
    pairing mode `complete_pairing` already stored it.
11. Send `connection.accepted`; register the connection.

The code is validated before the challenge but consumed only after the
signature and rotation checks pass. This prevents unauthenticated hello frames
from burning codes or pinning keys.

`MachineAuthenticator` additions:

```rust
pub struct MachineAuthState {
    pub pinned_key: Option<String>,
}

#[async_trait]
pub trait MachineAuthenticator {
    async fn pin_key(
        &self,
        machine_id: &str,
        public_key: &str,
    ) -> Result<(), ApiError>;
    async fn is_revoked(&self, machine_id: &str) -> Result<bool, ApiError>;

    /// Replaces the old `pinned_key` method.
    async fn machine_state(
        &self,
        machine_id: &str,
    ) -> Result<MachineAuthState, ApiError>;

    async fn machine_for_token(
        &self,
        token: &str,
    ) -> Result<Option<String>, ApiError>;

    async fn pairing_code(
        &self,
        code: &str,
    ) -> Result<PairingCodeStatus, ApiError>;

    async fn record_pairing_failure(
        &self,
        code_id: &str,
    ) -> Result<(), ApiError>;

    async fn complete_pairing(
        &self,
        request: PairingCompletion,
    ) -> Result<PairingOutcome, ApiError>;
}
```

- `machine_state` returns the pinned public key, if any.
- `pairing_code` takes the raw code, normalizes and hashes it internally, and
  returns the status defined in §9.1.
- Update `FnAuthenticator` and every test double for the new methods.
- `ConnectionGateway::new` no longer takes a machine `TokenAuth`; machine-token
  lookup goes through `MachineAuthenticator::machine_for_token`. The REST
  `TokenAuth` used for admin/user bearer authentication is unrelated and stays.
  `RegistryAuthenticator` implements the new methods over `AdminStorage`.

### 11.3 Per-IP throttling

- `apps/fleet-admin/src/main.rs` serves with
  `into_make_service_with_connect_info::<SocketAddr>()`.
- `machine_upgrade` extracts the peer address (a small `PeerAddr` extractor
  that falls back to `127.0.0.1` when `ConnectInfo` is absent in tests).
- The gateway keeps a `DashMap<IpAddr, VecDeque<Instant>>` and rejects more
  than 30 upgrade attempts per 10 minutes with HTTP 429.
- Behind a reverse proxy the peer is the proxy. Accurate per-client limits
  require a trusted-proxy configuration and are out of scope (§19).
- Per-code attempts (max 5) remain the primary brute-force defence.

### 11.4 Pairing code entropy and hashing

- 12 characters from the Crockford Base32 alphabet (no `I`, `L`, `O`, `U`),
  displayed as `XXXX-XXXX-XXXX`.
- Approximately 60 bits of entropy. This is a deliberate tightening of the
  “e.g. 10 characters / ~50 bits” example in `docs/connectivity.md`; 60 bits
  makes offline guessing of a leaked hash impractical within the TTL.
- Normalization: uppercase, remove `-` and whitespace, map `I`/`L` to `1`,
  `O` to `0`.
- At rest: `SHA-256(normalized_code)` with a unique index.
- TTL: 600 seconds by default.
- Single use; max 5 attempts.

## 12. Configuration

`crates/core-config/src/lib.rs`:

```toml
[auth]
admin_token = "..."                  # required
pair_code_ttl_secs = 600
pair_code_max_attempts = 5

[connection]
server_url = "ws://127.0.0.1:8787/ws/machine"
pairing_code = "..."                 # optional, automated provisioning
```

Environment variables:

| Variable | Purpose |
|---|---|
| `AGENT_FLEET_ADMIN_TOKEN` | REST admin token, unchanged |
| `AGENT_FLEET_PAIR_CODE_TTL` | overrides `pair_code_ttl_secs` |
| `AGENT_FLEET_PAIR_CODE` | daemon-only pairing code for automated provisioning |

Removed in this iteration:

- `auth.machine_token` and `AGENT_FLEET_MACHINE_TOKEN`;
- `connection.token` and `AGENT_FLEET_TOKEN`;
- `accept_shared_machine_token` and
  `AGENT_FLEET_ACCEPT_SHARED_MACHINE_TOKEN`;
- the machine-token branches and helpers in `core-auth`
  (`MACHINE_TOKEN_ENV`, `machine_token_env`, and the machine branch of
  `TokenAuth::from_env`). `TokenAuth` stays for REST bearer authentication.

Admin startup validation (`apps/fleet-admin/src/main.rs:105`) only requires a
non-empty `auth.admin_token`. There is no machine-token configuration to
validate.

## 13. Daemon changes

### 13.1 Per-machine token file

`crates/core-auth`:

```rust
pub fn generate_machine_token() -> String;              // afm_<64 hex>
pub fn machine_token_path(key_file: &Path) -> PathBuf;  // <key_file>.token
pub fn load_machine_token(path: &Path) -> Result<Option<String>>;
pub fn save_machine_token(path: &Path, token: &str) -> Result<()>;
pub fn clear_machine_token(path: &Path) -> Result<()>;
```

- Files are mode `0600`; loading rejects group/world-readable files using the
  same permission check as `MachineIdentity`.
- The file contains the token followed by a newline.

`apps/fleet-daemon/src/app.rs` reads the token only from
`<key_file>.token`. There is no `connection.token`, no `AGENT_FLEET_TOKEN`
fallback, and no shared token.

The daemon never logs the token.

### 13.2 Connection startup

`apps/fleet-daemon/src/app.rs:59` currently requires both `server_url` and
`token`. Change to:

- connect when `server_url` is present;
- `token` may be absent only while pairing; a paired machine must have a token;
- `pairing_code` may be present (`connection.pairing_code` or
  `AGENT_FLEET_PAIR_CODE`).

`ClientConfig` additions:

```rust
pub pairing_code: Option<String>,
pub token_file: Option<PathBuf>,
```

`ClientConfig.token` stays a `String`; an empty value means “no Authorization
header”. This avoids changing every existing constructor call.

Client behaviour:

- token present and non-empty => send `Authorization: Bearer <token>`;
- token absent and pairing code present => no Authorization header; include
  the code in `hello`;
- `connection.accepted.machine_token` present =>
  - save it to `token_file` (0600);
  - replace the in-memory token so reconnects use it;
  - clear the in-memory pairing code; remove `connection.pairing_code` from the
    config when convenient (it is single-use and expires, but it is still a
    secret at rest);
  - if saving fails, terminate the connection with a clear error: a token that
    cannot be persisted would be lost on restart.

An HTTP 401 during the upgrade means the per-machine token is invalid or the
machine was revoked. Treat it as terminal and require re-pairing.

Terminal errors (`NotPaired`, `TokenRequired`, `PairingCodeInvalid`) stop the
reconnect loop with a message like:

```text
machine is not paired; run `af setup` or `fleet-daemon pair --code ...`
```

### 13.3 `fleet-daemon pair`

`apps/fleet-daemon/src/main.rs` adds:

```bash
fleet-daemon pair \
  --server ws://127.0.0.1:8787/ws/machine \
  --code 7Q2M-9XKR-4TWD \
  [--config <toml>] [--name <name>] [--write-config]
```

`apps/fleet-daemon/src/bootstrap.rs`:

1. load config, resolve paths, load/create key and machine id;
2. call `fleet_core_connection::pair_machine(...)`;
3. save the token to `<key_file>.token` (0600);
4. print `machine_id`, `org_id`, token file path;
5. with `--write-config`, set `[connection] server_url` and `[machine] id`.

### 13.4 Shared pairing client

`crates/core-connection/src/client.rs` adds a one-shot function:

```rust
pub struct PairingConfig {
    pub server_url: String,
    pub machine_id: String,
    pub machine_name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub protocol_version: u16,
    pub pairing_code: String,
    pub handshake_timeout: Duration,
}

pub struct PairingResult {
    pub machine_id: String,
    pub org_id: String,
    pub token: String,
}

pub async fn pair_machine(
    config: PairingConfig,
    identity: &MachineIdentity,
) -> Result<PairingResult>;
```

Implementation: connect without Authorization, send hello with the code, sign
the challenge, read `connection.accepted`, require `machine_token`, return it.
No background heartbeat or reconnect tasks. `fleet-daemon pair` and
`af setup` share this function.

## 14. CLI changes

### 14.1 Dependencies

`apps/fleet-cli/Cargo.toml` adds:

```toml
fleet-core-common.workspace = true
fleet-core-auth.workspace = true
fleet-core-config.workspace = true
fleet-core-protocol.workspace = true
fleet-core-connection.workspace = true
```

This lets `af setup` reuse the pairing handshake and key/config helpers instead
of shelling out to `fleet-daemon`.

### 14.2 Commands

```bash
# Primary same-host / same-LAN flow
af setup \
  --url http://127.0.0.1:8787 \
  --token $AGENT_FLEET_ADMIN_TOKEN \
  --org org_default \
  --server ws://127.0.0.1:8787/ws/machine \
  --config ~/.agent-fleet/config.toml \
  --name workstation-01

# Admin-side code creation for the low-level daemon command
af machines pair-code --org org_default
```

`af setup` steps:

1. Resolve admin URL and token (flag/env/`cli.toml`); the token is required.
2. `GET /api/v1/health`; unreachable => fail with instructions.
3. Load the daemon config (`--config`), resolve `machine.key_file`; load or
   create the key and machine id. Move `load_or_create_machine_id` from
   `apps/fleet-daemon/src/services.rs` into `core-auth` so both binaries share
   it.
4. Call `POST /api/v1/pairing/codes`.
5. `core_connection::pair_machine(...)`.
6. `save_machine_token(<key_file>.token, token)`.
7. Edit the daemon TOML with `toml_edit`, preserving comments and backing up
   the file:
   - `[connection] server_url = "<ws_url>"`;
   - remove any legacy `[connection] token` and `[connection] pairing_code`;
   - `[machine] id = "<machine_id>"`;
   - `[machine] name = "<name>"` when explicitly provided.
8. Print machine id, organization, config path and the next step (start the
   daemon).
9. `--store-token` optionally saves the admin token to `cli.toml` (0600).
   Default: do not store it.

Safety checks:

- `ws://` to a non-loopback host is refused unless `--allow-insecure` is
  passed. Loopback means `localhost`, `127.0.0.1` or `::1`. Same-LAN plaintext
  is allowed only with this flag and only on a trusted network; use WSS
  otherwise.
- The admin token is never written to the daemon config.
- `--configure-only` writes configuration without pairing (for a machine that
  already has a token).
- Run `af setup` as the same user that runs the daemon service; the token file
  is owned by that user.

Idempotency:

- same key: re-pairs and rotates the token;
- different key: `machine_key_mismatch`; instruct the operator to revoke and
  unrevoke, or use the key-rotation flow.

### 14.3 `af machines pair-code`

Calls `POST /api/v1/pairing/codes` and prints the code and expiry. This is the
documented admin-side creation path for `fleet-daemon pair`. It is limited to
the same-host / same-LAN scenario; remote delivery is out of scope. It warns
that the code is shown once and should not be pasted into shell history.

## 15. Security analysis

| Threat | Mitigation |
|---|---|
| One leaked credential grants access to every machine | There is no shared machine token; every machine has its own token. |
| Stolen pairing code | 60-bit code, 600 s TTL, single use, 5 attempts, generic errors, per-IP throttle. |
| Code replay | Atomic single-use consumption; `used_at` checked inside the transaction. |
| Attacker pins their key with a stolen code | The code still requires a valid challenge signature; a code alone cannot complete pairing. |
| Attacker with a stolen machine token | Token is bound to `machine_id`; `hello.machine_id` must match; key rotation requires a proof signed by the pinned key. |
| Token at rest on the daemon | `<key_file>.token` mode 0600; group/world-readable files are rejected. |
| Token/code in logs | Manual `Debug` redaction; no full-struct logging; audit records ids and fingerprints only. |
| Re-pairing replaces the key | A different key still requires `KeyRotationProof` from the previously pinned key. |
| Revoked machine resurrects | Revocation checks before and inside the pairing transaction; `machine_revocations` survives machine-row deletion. |
| Brute-force pairing endpoint | Per-code attempts, 60-bit entropy, per-IP upgrade throttle; reverse-proxy caveat documented. |
| Plaintext transport | `af setup` refuses non-loopback `ws://`; docs require WSS outside loopback. |

Residual risks:

- A stolen per-machine token plus the machine's key file is equivalent to
  machine compromise; revoke the machine.
- Per-IP throttling is approximate behind a proxy.
- Code entropy is 60 bits; if an attacker obtains the database within the TTL,
  offline guessing is still bounded but not impossible at very large scale.
  A server-side pepper (HMAC) is the follow-up if this becomes a concern.

## 16. Removed paths and rollout

The product has not been launched, so this iteration does not keep any
compatibility path.

Removed:

- shared `AGENT_FLEET_MACHINE_TOKEN` / `auth.machine_token`;
- `AGENT_FLEET_TOKEN`, `connection.token`, and the machine-token resolution
  fallback;
- trust-on-first-use pinning on first connection;
- key-only connections for previously pinned machines;
- `accept_shared_machine_token`.

Every machine must run `af setup` (or `fleet-daemon pair`) before it can
connect. An unpaired machine receives `machine_not_paired`; a paired machine
without its token receives `machine_token_required`.

Rollout:

1. Deploy the admin with pairing endpoints, schema 0003 and the new gateway.
2. Pair every daemon with `af setup`; the daemon saves `<key_file>.token`.
3. Start the daemon and confirm it connects with its per-machine token.

Install scripts:

- `scripts/install/install.sh:371` stops generating and distributing the shared
  token;
- after the admin starts, the installer calls `af setup` with the admin token.

## 17. Testing

### core-auth

- code generation length/alphabet, normalization, hash stability;
- machine token format and hash;
- token file round-trip, 0600 mode, rejection of group/world-readable files;
- machine id persistence.

### admin-storage

- create/lookup/consume code;
- expired, used, locked, unknown code;
- concurrent consumption: exactly one success;
- second machine id using the same code fails;
- token lookup;
- re-pairing same key rotates the token hash;
- different key without rotation proof fails;
- organization mismatch fails;
- revoked machine fails;
- audit rows for creation, success and denied attempts.

### fleet-admin / gateway

- pairing handshake: hello with code => accepted with token;
- subsequent connection with the token => online;
- no token, no code, no pinned key => `machine_not_paired`;
- no token, no code, pinned key exists => `machine_token_required`;
- invalid Authorization header => 401;
- invalid/expired/used code => `pairing_code_invalid`;
- wrong signature increments attempts and eventually locks;
- re-pairing same key rotates the token; old token fails;
- different key requires rotation proof;
- per-IP upgrade throttle returns 429.

### fleet-daemon

- `pair` saves a 0600 token file;
- serve reads the token file;
- `connection.pairing_code` / `AGENT_FLEET_PAIR_CODE` auto-pairs on first
  connection;
- terminal errors stop reconnects;
- token file permission failure is reported clearly.

### fleet-cli

- `af setup` creates a code, pairs, writes the token file, removes legacy
  `connection.token`;
- `af machines pair-code` prints a code and expiry;
- server URL derivation from `--url`;
- non-loopback `ws://` refused without `--allow-insecure`;
- TOML backup and comment preservation;
- idempotent re-pairing;
- unreachable admin fails with a clear message.

### end-to-end

`scripts/test/e2e_smoke.sh`:

- admin + `af setup` + daemon serve => machine online;
- an unpaired daemon is rejected with `machine_not_paired`.

## 18. Documentation updates

- `docs/connectivity.md` §2: mark same-host/same-LAN as implemented; document
  token files, re-pairing, the removal of the shared token, and the
  reverse-proxy throttling limit.
- `docs/security.md`: replace the shared-token section with pairing +
  per-machine tokens; update known limitations.
- `docs/protocols.md`: new `connection.hello` / `connection.accepted` fields
  and error codes.
- `docs/development.md`, `README.md`: quick start uses `af setup`.
- `docs/architecture.md`, `docs/crate-interfaces.md`, `docs/mvp-status.md`.
- `apps/docs/src/content/docs/how-it-works/handshake.mdx`,
  `connection.mdx`, `registration.mdx`.
- Document the non-goals from §4 and the future work below.

## 19. Future work

Recommended order:

1. Remote code handoff UX (QR / one-line installer / guidance for
   `af machines pair-code` + `fleet-daemon pair`).
2. Deferred enrollment for machines that are not yet reachable, with a
   longer-lived or single-use provisioning credential and an explicit
   security trade-off.
3. Trusted-proxy configuration and accurate per-IP throttling.
4. Pairing page in the web UI.
5. OAuth device flow for organizations that want browser approval / SSO.
6. Independent token rotation and expiry.
7. mTLS enrollment for private PKI deployments.
8. Multiple admin instances with connection leases.

## 20. Implementation order

1. **Protocol and primitives**: hello/accepted fields, redacted `Debug`,
   error codes, code/token generation, token file helpers, machine id helper.
2. **Storage**: migration 0003, pairing codes repository, machine pairing,
   token lookup, audit.
3. **Admin API and configuration**: `POST /api/v1/pairing/codes`, remove the
   shared-token config and environment variables, relax startup validation,
   audit.
4. **Gateway**: machine-token upgrade auth, pairing handshake, token issuance,
   per-IP throttle; delete the shared-token and TOFU paths.
5. **Daemon**: token file, `fleet-daemon pair`, auto-pair, terminal errors;
   remove `connection.token` / `AGENT_FLEET_TOKEN`.
6. **CLI**: dependencies, `af setup`, `af machines pair-code`, TOML edits,
   safety checks.
7. **Docs, scripts and e2e**: documentation, installer, smoke tests.

## 21. Decisions

| Question | Decision |
|---|---|
| Long-lived credential | Per-machine token, as specified in `docs/connectivity.md` §2. |
| Where pairing happens | In the existing `/ws/machine` handshake, not a REST redeem endpoint. |
| When the code is consumed | Only after the challenge signature and rotation checks pass. |
| Code length | 12 Crockford Base32 characters (~60 bits), grouped 4-4-4. |
| Token storage | `<key_file>.token`, mode 0600, next to the key. |
| Shared machine token | Removed entirely; no fallback. |
| TOFU / key-only compatibility | Removed; every machine must be paired. |
| Backward compatibility | None; the product is not launched. |
| Scope | Same host / same LAN only; fails clearly when the admin is unreachable. |
| Admin UI | Not in this iteration. |

There are no open questions blocking implementation.
