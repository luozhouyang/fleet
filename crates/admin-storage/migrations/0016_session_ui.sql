-- Per-user session presentation state.
--
-- Sessions are visible to the whole organization; pinning, archiving, labels,
-- folders and read markers are personal. Keeping them in their own tables is
-- what protects them from daemon reconciliation: `Sessions::upsert_on` writes
-- an explicit column list on `agent_sessions` and can never touch these rows.

CREATE TABLE IF NOT EXISTS session_ui_state (
    actor_id    TEXT NOT NULL,
    session_id  TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    pinned      INTEGER NOT NULL DEFAULT 0,
    pinned_rank INTEGER,
    archived    INTEGER NOT NULL DEFAULT 0,
    labels_json TEXT NOT NULL DEFAULT '[]',
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (actor_id, session_id)
);

CREATE INDEX IF NOT EXISTS idx_session_ui_state_pinned
    ON session_ui_state(actor_id, pinned, pinned_rank);

CREATE TABLE IF NOT EXISTS session_reads (
    actor_id     TEXT NOT NULL,
    session_id   TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    last_read_at TEXT NOT NULL,
    PRIMARY KEY (actor_id, session_id)
);

CREATE TABLE IF NOT EXISTS session_folders (
    id         TEXT PRIMARY KEY,
    actor_id   TEXT NOT NULL,
    name       TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_session_folders_actor
    ON session_folders(actor_id, sort_order);

CREATE TABLE IF NOT EXISTS session_folder_members (
    actor_id   TEXT NOT NULL,
    folder_id  TEXT NOT NULL REFERENCES session_folders(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    PRIMARY KEY (actor_id, folder_id, session_id)
);

CREATE INDEX IF NOT EXISTS idx_session_folder_members_session
    ON session_folder_members(actor_id, session_id);

-- Agent activity watermark, advanced by the admin's event projection. It is
-- deliberately absent from `Sessions::upsert_on`, so a daemon reconciliation
-- that rewrites the session row cannot move it backwards.
ALTER TABLE agent_sessions ADD COLUMN last_activity_at TEXT;
