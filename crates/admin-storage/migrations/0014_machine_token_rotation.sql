-- Machine token rotation is two-phase: the daemon is offered a new token over
-- its authenticated connection and only reports success after persisting it.
-- Until the admin records that confirmation, the previous token stays valid
-- and the pending hash is accepted too, so a lost confirmation frame can
-- never lock a daemon out of the admin.

ALTER TABLE machines ADD COLUMN pending_token_hash TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_machines_pending_token_hash
    ON machines(pending_token_hash)
    WHERE pending_token_hash IS NOT NULL;
