-- Multi-organization identity model.
--
-- Users no longer carry a single organization and role. Membership lives in
-- `memberships`, so one user can belong to any number of organizations with a
-- different role in each. Human authentication (OAuth identities, browser
-- sessions) is introduced here as well; agent sessions stay in `agent_sessions`.
--
-- Deletion is deliberately non-cascading for organizations: dropping an
-- organization must never delete machines, workspaces, agent sessions or audit
-- history. `machine_orgs` therefore uses ON DELETE SET NULL, and the one row a
-- user or organization owns (identities, sessions, memberships) is all that is
-- removed.

-- 1. Organization metadata. Columns are added without foreign keys because
--    rebuilding the `organizations` table would cascade into every child table.
--    `owner_user_id` is validated in code.
ALTER TABLE organizations ADD COLUMN kind TEXT NOT NULL DEFAULT 'team';
ALTER TABLE organizations ADD COLUMN owner_user_id TEXT;
ALTER TABLE organizations ADD COLUMN deleted_at TEXT;
ALTER TABLE organizations ADD COLUMN updated_at TEXT;
UPDATE organizations SET updated_at = created_at WHERE updated_at IS NULL;

-- 2. Preserve the legacy user -> organization mapping before rebuilding users.
CREATE TABLE _legacy_user_memberships (
    user_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO _legacy_user_memberships (user_id, org_id, role, created_at, updated_at)
SELECT id, org_id, role, created_at, updated_at FROM users;

-- 3. Rebuild `users` without org_id/role and with an OAuth-friendly shape.
--    Nothing references `users` yet, so this is safe with foreign keys on.
CREATE TABLE users_new (
    id TEXT PRIMARY KEY,
    email TEXT,
    name TEXT NOT NULL,
    avatar_url TEXT,
    token_hash TEXT,
    token_issued_at TEXT,
    token_expires_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    disabled_at TEXT
);
INSERT INTO users_new (
    id, email, name, avatar_url, token_hash, token_issued_at, token_expires_at,
    created_at, updated_at, disabled_at
)
SELECT id, NULL, name, NULL, token_hash, token_issued_at, token_expires_at,
       created_at, updated_at, NULL
FROM users;
DROP TABLE users;
ALTER TABLE users_new RENAME TO users;
CREATE UNIQUE INDEX idx_users_email ON users(email) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX idx_users_token_hash ON users(token_hash) WHERE token_hash IS NOT NULL;

-- 4. Membership is the sole source of a user's role inside an organization.
CREATE TABLE IF NOT EXISTS memberships (
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    invited_by TEXT,
    PRIMARY KEY (org_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_memberships_user ON memberships(user_id);

INSERT OR IGNORE INTO memberships (org_id, user_id, role, created_at, updated_at)
SELECT org_id, user_id, role, created_at, updated_at FROM _legacy_user_memberships;
DROP TABLE _legacy_user_memberships;

-- 5. External identities. A user may link GitHub and Google when both report
--    the same verified email.
CREATE TABLE IF NOT EXISTS auth_identities (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    subject TEXT NOT NULL,
    email TEXT NOT NULL,
    login TEXT,
    avatar_url TEXT,
    created_at TEXT NOT NULL,
    last_login_at TEXT,
    UNIQUE(provider, subject)
);
CREATE INDEX IF NOT EXISTS idx_auth_identities_user ON auth_identities(user_id);

-- 6. Browser sessions. Only the hash of the session token is stored.
CREATE TABLE IF NOT EXISTS auth_sessions (
    id_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    user_agent TEXT,
    ip TEXT,
    revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_user ON auth_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_expiry ON auth_sessions(expires_at);

-- 7. Single-use OAuth authorization state. Rows are consumed exactly once.
CREATE TABLE IF NOT EXISTS auth_oauth_states (
    state_hash TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    nonce TEXT,
    redirect_uri TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_auth_oauth_states_expiry ON auth_oauth_states(expires_at);

-- 8. Organization invitations. Delivery is out of band; the token is stored
--    hashed and accepted on first sign-in with a matching verified email.
CREATE TABLE IF NOT EXISTS org_invitations (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    role TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    accepted_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_org_invitations_org ON org_invitations(org_id);
CREATE INDEX IF NOT EXISTS idx_org_invitations_email ON org_invitations(email);

-- 9. Deleting an organization must not delete its machines. Ownership becomes
--    unassigned instead, and an operator can claim the machine later.
CREATE TABLE machine_orgs_new (
    machine_id TEXT PRIMARY KEY REFERENCES machines(id) ON DELETE CASCADE,
    org_id TEXT REFERENCES organizations(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL
);
INSERT INTO machine_orgs_new (machine_id, org_id, created_at)
SELECT machine_id, org_id, created_at FROM machine_orgs;
DROP TABLE machine_orgs;
ALTER TABLE machine_orgs_new RENAME TO machine_orgs;
CREATE INDEX idx_machine_orgs_org ON machine_orgs(org_id);
