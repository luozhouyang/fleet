CREATE TABLE IF NOT EXISTS question_requests (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    turn_id TEXT,
    tool_call_id TEXT,
    questions_json TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT,
    expires_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_questions_session ON question_requests(session_id);
CREATE INDEX IF NOT EXISTS idx_questions_status ON question_requests(status);
