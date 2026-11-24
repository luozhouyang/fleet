-- Bind a realtime ticket to the browser session that minted it.
--
-- Revoking the session (logout, password rotation, disabled account) must also
-- stop an event socket that was upgraded with a ticket minted by that session.
-- Tickets minted with a bearer token have no session and keep the token-based
-- reauthentication path.
ALTER TABLE realtime_tickets ADD COLUMN session_hash TEXT;
