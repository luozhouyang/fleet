-- Remember which organization owned a machine when it was revoked. The
-- machine row and its machine_orgs entry are deleted on revocation, so without
-- this column an organization administrator could list or unrevoke another
-- organization's machines by id.

ALTER TABLE machine_revocations ADD COLUMN org_id TEXT;
CREATE INDEX IF NOT EXISTS idx_machine_revocations_org ON machine_revocations(org_id);
