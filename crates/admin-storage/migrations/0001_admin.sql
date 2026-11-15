-- Admin control-plane schema. No transcript tables live here.

DROP TABLE IF EXISTS agent_tool_calls;
DROP TABLE IF EXISTS agent_messages;
DROP TABLE IF EXISTS agent_turns;
DROP TABLE IF EXISTS workspace_entries;

CREATE TABLE IF NOT EXISTS storage_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO storage_meta (key, value) VALUES ('role', 'admin');

CREATE TABLE IF NOT EXISTS organizations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL
);

INSERT OR IGNORE INTO organizations (id, name, created_at)
VALUES ('org_default', 'Default', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    role TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_users_org ON users(org_id);

CREATE TABLE IF NOT EXISTS machines (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    hostname TEXT NOT NULL DEFAULT '',
    os TEXT NOT NULL DEFAULT '',
    arch TEXT NOT NULL DEFAULT '',
    daemon_version TEXT NOT NULL DEFAULT '',
    protocol_version INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'offline',
    public_key TEXT,
    connected_at TEXT,
    last_seen_at TEXT,
    providers_json TEXT NOT NULL DEFAULT '[]',
    workspaces_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS machine_revocations (
    machine_id TEXT PRIMARY KEY,
    revoked_at TEXT NOT NULL,
    reason TEXT
);

CREATE TABLE IF NOT EXISTS machine_orgs (
    machine_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_machine_orgs_org ON machine_orgs(org_id);

CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL,
    project_id TEXT,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL,
    is_git_repository INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_workspaces_machine ON workspaces(machine_id);

CREATE TABLE IF NOT EXISTS agent_sessions (
    id TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL,
    workspace_id TEXT,
    provider TEXT NOT NULL,
    agent_type TEXT NOT NULL,
    status TEXT NOT NULL,
    cwd TEXT NOT NULL,
    title TEXT,
    last_error_json TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_machine ON agent_sessions(machine_id);

CREATE TABLE IF NOT EXISTS permission_requests (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_id TEXT,
    tool_call_id TEXT,
    tool_name TEXT NOT NULL,
    description TEXT NOT NULL,
    input_json TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT,
    expires_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_permissions_session ON permission_requests(session_id);
CREATE INDEX IF NOT EXISTS idx_permissions_status ON permission_requests(status);

CREATE TABLE IF NOT EXISTS audit_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    actor TEXT NOT NULL,
    org_id TEXT,
    action TEXT NOT NULL,
    subject TEXT NOT NULL,
    details_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_org ON audit_logs(org_id, id);
