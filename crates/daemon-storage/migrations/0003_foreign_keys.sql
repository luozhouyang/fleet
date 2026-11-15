-- Enforce transcript integrity with SQLite foreign keys.
--
-- Child tables are rebuilt in dependency order because SQLite cannot add
-- constraints with ALTER TABLE. Orphan rows from older versions are cleaned
-- first. `event_outbox` intentionally has no session foreign key: it must
-- outlive the session so `session.deleted` and earlier pending events can be
-- delivered or explicitly discarded.

-- 1. Clean data that would violate the new constraints.
UPDATE agent_sessions
SET workspace_id = NULL
WHERE workspace_id = ''
   OR (workspace_id IS NOT NULL
       AND workspace_id NOT IN (SELECT id FROM workspaces));

-- Preserve transcript rows even when the pre-split database lost their
-- session row. A placeholder is safer than deleting user history; the daemon
-- reports it as failed/adopted and never resumes it.
INSERT INTO agent_sessions (
    id, machine_id, workspace_id, provider, agent_type, status, cwd, title,
    last_error_json, metadata_json, created_at, updated_at
)
SELECT session_id,
       COALESCE(
           (SELECT machine_id FROM workspaces ORDER BY created_at LIMIT 1),
           (SELECT machine_id FROM agent_sessions LIMIT 1),
           ''
       ),
       NULL,
       'unknown',
       'unknown',
       'failed',
       '',
       NULL,
       NULL,
       '{"adopted":true}',
       MIN(at),
       MAX(at)
FROM (
    SELECT session_id, created_at AS at FROM agent_messages
    UNION ALL
    SELECT session_id, started_at AS at FROM agent_turns
    UNION ALL
    SELECT session_id, started_at AS at FROM agent_tool_calls
    UNION ALL
    SELECT session_id, created_at AS at FROM permission_requests
)
WHERE session_id NOT IN (SELECT id FROM agent_sessions)
GROUP BY session_id;

UPDATE agent_messages
SET turn_id = NULL
WHERE turn_id IS NOT NULL
  AND turn_id NOT IN (SELECT id FROM agent_turns);
UPDATE agent_tool_calls
SET turn_id = NULL
WHERE turn_id IS NOT NULL
  AND turn_id NOT IN (SELECT id FROM agent_turns);
UPDATE permission_requests
SET turn_id = NULL
WHERE turn_id IS NOT NULL
  AND turn_id NOT IN (SELECT id FROM agent_turns);
UPDATE permission_requests
SET tool_call_id = NULL
WHERE tool_call_id IS NOT NULL
  AND tool_call_id NOT IN (SELECT id FROM agent_tool_calls);

DELETE FROM workspace_entries
WHERE workspace_id NOT IN (SELECT id FROM workspaces);

-- 2. workspace_entries -> workspaces
CREATE TABLE workspace_entries_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
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
INSERT INTO workspace_entries_new (
    id, workspace_id, parent_path, path, name, entry_type, size, mtime_ns,
    git_status, ignored
)
SELECT id, workspace_id, parent_path, path, name, entry_type, size, mtime_ns,
       git_status, ignored
FROM workspace_entries;
DROP TABLE workspace_entries;
ALTER TABLE workspace_entries_new RENAME TO workspace_entries;
CREATE INDEX idx_workspace_parent
    ON workspace_entries(workspace_id, parent_path);

-- 3. agent_sessions -> workspaces
CREATE TABLE agent_sessions_new (
    id TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL,
    workspace_id TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
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
INSERT INTO agent_sessions_new (
    id, machine_id, workspace_id, provider, agent_type, status, cwd, title,
    last_error_json, metadata_json, created_at, updated_at
)
SELECT id, machine_id, workspace_id, provider, agent_type, status, cwd, title,
       last_error_json, metadata_json, created_at, updated_at
FROM agent_sessions;
DROP TABLE agent_sessions;
ALTER TABLE agent_sessions_new RENAME TO agent_sessions;
CREATE INDEX idx_sessions_machine ON agent_sessions(machine_id);
CREATE INDEX idx_sessions_workspace ON agent_sessions(workspace_id);

-- 4. agent_turns -> agent_sessions
CREATE TABLE agent_turns_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    error_json TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT
);
INSERT INTO agent_turns_new (
    id, session_id, status, error_json, started_at, completed_at
)
SELECT id, session_id, status, error_json, started_at, completed_at
FROM agent_turns;
DROP TABLE agent_turns;
ALTER TABLE agent_turns_new RENAME TO agent_turns;
CREATE INDEX idx_turns_session ON agent_turns(session_id);

-- 5. agent_messages -> agent_sessions / agent_turns
CREATE TABLE agent_messages_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    turn_id TEXT REFERENCES agent_turns(id) ON DELETE SET NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO agent_messages_new (
    id, session_id, turn_id, role, content, created_at
)
SELECT id, session_id, turn_id, role, content, created_at
FROM agent_messages;
DROP TABLE agent_messages;
ALTER TABLE agent_messages_new RENAME TO agent_messages;
CREATE INDEX idx_messages_session ON agent_messages(session_id);
CREATE INDEX idx_messages_turn ON agent_messages(turn_id);

-- 6. agent_tool_calls -> agent_sessions / agent_turns
CREATE TABLE agent_tool_calls_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    turn_id TEXT REFERENCES agent_turns(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    input_json TEXT NOT NULL DEFAULT '{}',
    output_json TEXT,
    error TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT
);
INSERT INTO agent_tool_calls_new (
    id, session_id, turn_id, name, status, input_json, output_json,
    error, started_at, completed_at
)
SELECT id, session_id, turn_id, name, status, input_json, output_json,
       error, started_at, completed_at
FROM agent_tool_calls;
DROP TABLE agent_tool_calls;
ALTER TABLE agent_tool_calls_new RENAME TO agent_tool_calls;
CREATE INDEX idx_tool_calls_session
    ON agent_tool_calls(session_id, started_at);
CREATE INDEX idx_tool_calls_turn ON agent_tool_calls(turn_id);

-- 7. permission_requests -> agent_sessions / turns / tool calls
CREATE TABLE permission_requests_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    turn_id TEXT REFERENCES agent_turns(id) ON DELETE SET NULL,
    tool_call_id TEXT REFERENCES agent_tool_calls(id) ON DELETE SET NULL,
    tool_name TEXT NOT NULL,
    description TEXT NOT NULL,
    input_json TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT,
    expires_at TEXT
);
INSERT INTO permission_requests_new (
    id, session_id, turn_id, tool_call_id, tool_name, description,
    input_json, status, created_at, resolved_at, expires_at
)
SELECT id, session_id, turn_id, tool_call_id, tool_name, description,
       input_json, status, created_at, resolved_at, expires_at
FROM permission_requests;
DROP TABLE permission_requests;
ALTER TABLE permission_requests_new RENAME TO permission_requests;
CREATE INDEX idx_permissions_session ON permission_requests(session_id);
CREATE INDEX idx_permissions_status ON permission_requests(status);
CREATE INDEX idx_permissions_turn ON permission_requests(turn_id);
CREATE INDEX idx_permissions_tool ON permission_requests(tool_call_id);
