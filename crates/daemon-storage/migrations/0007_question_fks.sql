-- Rebuild question_requests so turn and tool-call references cascade on
-- delete. SQLite cannot add foreign keys with ALTER TABLE, so copy the rows
-- into a new table. Dangling references from the old schema become NULL.
CREATE TABLE question_requests_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    turn_id TEXT REFERENCES agent_turns(id) ON DELETE SET NULL,
    tool_call_id TEXT REFERENCES agent_tool_calls(id) ON DELETE SET NULL,
    questions_json TEXT NOT NULL,
    answers_json TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT,
    expires_at TEXT
);

INSERT INTO question_requests_new (
    id, session_id, turn_id, tool_call_id, questions_json, answers_json,
    status, created_at, resolved_at, expires_at
)
SELECT
    id,
    session_id,
    (SELECT t.id FROM agent_turns AS t WHERE t.id = q.turn_id),
    (SELECT c.id FROM agent_tool_calls AS c WHERE c.id = q.tool_call_id),
    questions_json,
    answers_json,
    status,
    created_at,
    resolved_at,
    expires_at
FROM question_requests AS q;

DROP TABLE question_requests;
ALTER TABLE question_requests_new RENAME TO question_requests;

CREATE INDEX IF NOT EXISTS idx_questions_session ON question_requests(session_id);
CREATE INDEX IF NOT EXISTS idx_questions_status ON question_requests(status);
