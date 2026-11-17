-- Preserve the human-readable machine name across revocation. The machines
-- row is deleted when a machine is revoked, so without this column the
-- revoked-machines panel can only show the opaque machine id.

ALTER TABLE machine_revocations ADD COLUMN machine_name TEXT;
