-- Single-use WebSocket tickets.
--
-- The browser session cookie is HttpOnly and same-site, but a query string can
-- end up in logs and browser history. The event socket therefore consumes a
-- ticket: a random value with a 60-second TTL, bound to one user and one
-- organization, stored only as a hash and deleted on first use.
CREATE TABLE IF NOT EXISTS realtime_tickets (
    ticket_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_realtime_tickets_expiry ON realtime_tickets(expires_at);
