-- Retention support for single-use realtime tickets.
--
-- Tickets are minted for every browser event-socket connection and are pruned
-- after they are consumed or expire. The partial index keeps the consumed side
-- of that sweep cheap once a database has accumulated historical rows; the
-- expiry index exists in migration 0012.

CREATE INDEX IF NOT EXISTS idx_realtime_tickets_consumed
    ON realtime_tickets(consumed_at)
    WHERE consumed_at IS NOT NULL;
