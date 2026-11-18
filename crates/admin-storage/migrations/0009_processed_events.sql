-- Durable deduplication for applied agent events.
--
-- The admin keeps an in-process set as a hot cache; this table makes replay
-- deduplication survive an admin restart. An event is recorded here only
-- after its projection was applied, and the daemon cursor is acknowledged
-- only after this row is committed. Rows are pruned per machine so the table
-- stays bounded (see the storage module for the retention policy).
CREATE TABLE IF NOT EXISTS processed_agent_events (
    machine_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    processed_at TEXT NOT NULL,
    PRIMARY KEY (machine_id, event_id)
);

-- Pruning orders by rowid within one machine; SQLite indexes include the
-- rowid, so this index supports an index-only reverse scan.
CREATE INDEX IF NOT EXISTS idx_processed_agent_events_machine
    ON processed_agent_events(machine_id);
