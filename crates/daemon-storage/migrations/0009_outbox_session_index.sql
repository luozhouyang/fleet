-- Session-scoped replay pruning.
--
-- Deleting a session acknowledges and removes its earlier pending rows. The
-- composite index keeps that scan proportional to the session's backlog
-- instead of the whole outbox.

CREATE INDEX IF NOT EXISTS idx_outbox_session
    ON event_outbox(session_id, sequence);
