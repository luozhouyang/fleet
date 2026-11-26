CREATE TABLE IF NOT EXISTS agent_plans (
    session_id TEXT PRIMARY KEY REFERENCES agent_sessions(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    turn_id TEXT,
    native_plan_id TEXT,
    entries_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
