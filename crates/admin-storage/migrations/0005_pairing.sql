-- Pairing codes and per-machine token credentials.
--
-- Codes are stored hashed only; the plain text is shown to the operator once.
-- The machine token is likewise stored as a hash and resolved on every request.

CREATE TABLE IF NOT EXISTS pairing_codes (
    id                 TEXT PRIMARY KEY,
    code_hash          TEXT NOT NULL UNIQUE,
    org_id             TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    created_by         TEXT NOT NULL,
    machine_name_hint  TEXT,
    expires_at         TEXT NOT NULL,
    used_at            TEXT,
    used_by_machine_id TEXT,
    attempts           INTEGER NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pairing_codes_org ON pairing_codes(org_id);
CREATE INDEX IF NOT EXISTS idx_pairing_codes_expires ON pairing_codes(expires_at);

ALTER TABLE machines ADD COLUMN token_hash TEXT;
ALTER TABLE machines ADD COLUMN token_created_at TEXT;
ALTER TABLE machines ADD COLUMN paired_at TEXT;
ALTER TABLE machines ADD COLUMN paired_by TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_machines_token_hash
    ON machines(token_hash)
    WHERE token_hash IS NOT NULL;
