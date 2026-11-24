-- Idempotency keys for retried mutation RPCs.
--
-- A row is inserted with a NULL response while the first request runs, then
-- updated with the serialized response so a retry replays it instead of
-- executing twice. Expired rows are pruned on the next reservation.
CREATE TABLE IF NOT EXISTS idempotency_keys (
    key TEXT PRIMARY KEY,
    method TEXT NOT NULL,
    response TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_idempotency_keys_expires_at
    ON idempotency_keys (expires_at);
