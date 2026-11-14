-- Backfill machine ownership introduced in 0002 and scope audit records by org.

INSERT OR IGNORE INTO organizations (id, name, created_at)
VALUES ('org_default', 'Default', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

INSERT OR IGNORE INTO machine_orgs (machine_id, org_id, created_at)
SELECT id, 'org_default', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM machines;

ALTER TABLE audit_logs ADD COLUMN org_id TEXT;
CREATE INDEX IF NOT EXISTS idx_audit_org ON audit_logs(org_id, id);

-- Best-effort backfill for records whose subject still exists.
UPDATE audit_logs
SET org_id = (
    SELECT mo.org_id
    FROM workspaces w
    JOIN machine_orgs mo ON mo.machine_id = w.machine_id
    WHERE w.id = audit_logs.subject
)
WHERE org_id IS NULL AND subject IN (SELECT id FROM workspaces);

UPDATE audit_logs
SET org_id = (SELECT org_id FROM users WHERE users.id = audit_logs.subject)
WHERE org_id IS NULL AND subject IN (SELECT id FROM users);

UPDATE audit_logs
SET org_id = (SELECT org_id FROM machine_orgs WHERE machine_id = audit_logs.subject)
WHERE org_id IS NULL AND subject IN (SELECT machine_id FROM machine_orgs);
