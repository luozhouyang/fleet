-- Track when each user token was issued and when it expires.
-- `token_expires_at IS NULL` means the token never expires, preserving every
-- token that existed before this migration.
ALTER TABLE users ADD COLUMN token_issued_at TEXT;
ALTER TABLE users ADD COLUMN token_expires_at TEXT;
UPDATE users SET token_issued_at = created_at WHERE token_issued_at IS NULL;
