-- Preserve the pinned Ed25519 key across revocation. Without it, an explicit
-- unrevoke would re-enter trust-on-first-use and any holder of the shared
-- machine token could claim the same machine id with a different key.

ALTER TABLE machine_revocations ADD COLUMN public_key TEXT;
