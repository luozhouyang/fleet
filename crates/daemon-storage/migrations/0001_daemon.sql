-- Daemon schema. This database owns the authoritative agent transcript.

DROP TABLE IF EXISTS machine_revocations;
DROP TABLE IF EXISTS machine_orgs;
DROP TABLE IF EXISTS users;
DROP TABLE IF EXISTS organizations;
DROP TABLE IF EXISTS audit_logs;
DROP TABLE IF EXISTS machines;

CREATE TABLE IF NOT EXISTS storage_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO storage_meta (key, value) VALUES ('role', 'daemon');

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

CREATE TABLE IF NOT EXISTS workspace_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    parent_path TEXT NOT NULL,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    entry_type TEXT NOT NULL,
    size INTEGER,
    mtime_ns INTEGER,
    git_status TEXT,
    ignored INTEGER NOT NULL DEFAULT 0,
    UNIQUE(workspace_id, path)
);
CREATE INDEX IF NOT EXISTS idx_workspace_parent ON workspace_entries(workspace_id, parent_path);

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

CREATE TABLE IF NOT EXISTS agent_turns (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    status TEXT NOT NULL,
    error_json TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_turns_session ON agent_turns(session_id);

CREATE TABLE IF NOT EXISTS agent_messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_id TEXT,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON agent_messages(session_id);

CREATE TABLE IF NOT EXISTS agent_tool_calls (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_id TEXT,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    input_json TEXT NOT NULL DEFAULT '{}',
    output_json TEXT,
    error TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_tool_calls_session ON agent_tool_calls(session_id, started_at);

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

CREATE TABLE IF NOT EXISTS event_outbox (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_outbox_sequence ON event_outbox(sequence);

CREATE TABLE IF NOT EXISTS event_cursors (
    consumer TEXT PRIMARY KEY,
    acked_sequence INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);
INSERT OR IGNORE INTO event_cursors (consumer, acked_sequence, updated_at)
VALUES ('admin', 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
