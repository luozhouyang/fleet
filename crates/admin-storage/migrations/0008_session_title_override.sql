-- A session title set by an operator must survive daemon snapshots, which
-- carry the provider's own title (often empty). Upserts preserve the stored
-- title while this flag is set; clearing the title unlocks it again.

ALTER TABLE agent_sessions ADD COLUMN title_override INTEGER NOT NULL DEFAULT 0;
