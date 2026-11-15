-- Out-of-order acknowledgements from the admin.
--
-- The admin acknowledges each applied outbox sequence. Events can arrive out
-- of order after a replay, so acknowledged sequences above a gap are retained
-- until the missing sequence arrives; only then does the contiguous cursor
-- advance and prune the outbox.

CREATE TABLE IF NOT EXISTS event_acks (
    sequence INTEGER PRIMARY KEY,
    acked_at TEXT NOT NULL
);
