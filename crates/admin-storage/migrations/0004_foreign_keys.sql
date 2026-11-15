-- Enforce relational integrity with SQLite foreign keys.
--
-- SQLite cannot add constraints with ALTER TABLE, so the affected child tables
-- are rebuilt in dependency order. Orphan rows left by older versions are
-- cleaned first; otherwise the constrained copies would fail to insert.

-- 1. Clean data that would violate the new constraints.
DELETE FROM agent_sessions
WHERE machine_id NOT IN (SELECT id FROM machines);

UPDATE agent_sessions
SET workspace_id = NULL
WHERE workspace_id = ''
   OR (workspace_id IS NOT NULL
       AND workspace_id NOT IN (SELECT id FROM workspaces));

DELETE FROM workspaces
WHERE machine_id NOT IN (SELECT id FROM machines);

DELETE FROM permission_requests
WHERE session_id NOT IN (SELECT id FROM agent_sessions);

DELETE FROM machine_orgs
WHERE machine_id NOT IN (SELECT id FROM machines)
   OR org_id NOT IN (SELECT id FROM organizations);

-- 2. workspaces -> machines
CREATE TABLE workspaces_new (
    id TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    project_id TEXT,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL,
    is_git_repository INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO workspaces_new (
    id, machine_id, project_id, name, root_path, is_git_repository,
    created_at, updated_at
)
SELECT id, machine_id, project_id, name, root_path, is_git_repository,
       created_at, updated_at
FROM workspaces;
DROP TABLE workspaces;
ALTER TABLE workspaces_new RENAME TO workspaces;
CREATE INDEX idx_workspaces_machine ON workspaces(machine_id);

-- 3. agent_sessions -> machines / workspaces
CREATE TABLE agent_sessions_new (
    id TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
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

-- 4. permission_requests -> agent_sessions
CREATE TABLE permission_requests_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
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

-- 5. machine_orgs -> machines / organizations
CREATE TABLE machine_orgs_new (
    machine_id TEXT PRIMARY KEY REFERENCES machines(id) ON DELETE CASCADE,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL
);
INSERT INTO machine_orgs_new (machine_id, org_id, created_at)
SELECT machine_id, org_id, created_at
FROM machine_orgs;
DROP TABLE machine_orgs;
ALTER TABLE machine_orgs_new RENAME TO machine_orgs;
CREATE INDEX idx_machine_orgs_org ON machine_orgs(org_id);
