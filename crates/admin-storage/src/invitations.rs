//! Organization invitations.
//!
//! Delivery is out of band: the API returns a plaintext token exactly once and
//! stores only its hash. An invitation is bound to one email address and is
//! accepted when a user with that verified email signs in.

use fleet_core_common::{new_id, now, Timestamp};
use fleet_core_protocol::authz::{OrgInvitation, UserRole};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::users::normalize_email;
use crate::Result;

/// Handle for invitation queries.
#[derive(Clone)]
pub struct Invitations {
    pool: SqlitePool,
}

impl Invitations {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Invitations {
    /// Create an invitation. The caller generates and returns the plaintext
    /// token exactly once; only `token_hash` is stored.
    pub async fn create(
        &self,
        org_id: &str,
        email: &str,
        role: UserRole,
        token_hash: &str,
        created_by: &str,
        expires_at: Timestamp,
    ) -> Result<OrgInvitation> {
        let timestamp = now();
        let invitation = OrgInvitation {
            id: new_id("invite"),
            org_id: org_id.to_string(),
            email: normalize_email(email),
            role,
            created_by: created_by.to_string(),
            created_at: timestamp,
            expires_at,
            accepted_at: None,
        };
        sqlx::query(
            r#"
            INSERT INTO org_invitations
                (id, org_id, email, role, token_hash, created_by, created_at, expires_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )
        .bind(&invitation.id)
        .bind(&invitation.org_id)
        .bind(&invitation.email)
        .bind(role.as_str())
        .bind(token_hash)
        .bind(&invitation.created_by)
        .bind(format_ts(timestamp))
        .bind(format_ts(expires_at))
        .execute(&self.pool)
        .await?;
        Ok(invitation)
    }

    /// List pending (unaccepted, unexpired) invitations for an organization.
    pub async fn pending_for_org(&self, org_id: &str) -> Result<Vec<OrgInvitation>> {
        let rows: Vec<InvitationRow> = sqlx::query_as(
            r#"
            SELECT id, org_id, email, role, created_by, created_at, expires_at, accepted_at
            FROM org_invitations
            WHERE org_id = ?1 AND accepted_at IS NULL AND expires_at > ?2
            ORDER BY created_at
            "#,
        )
        .bind(org_id)
        .bind(format_ts(now()))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| OrgInvitation::try_from(row).ok())
            .collect())
    }

    /// Pending invitations bound to one email, across every organization.
    pub async fn pending_for_email(&self, email: &str) -> Result<Vec<OrgInvitation>> {
        let rows: Vec<InvitationRow> = sqlx::query_as(
            r#"
            SELECT id, org_id, email, role, created_by, created_at, expires_at, accepted_at
            FROM org_invitations
            WHERE email = ?1 AND accepted_at IS NULL AND expires_at > ?2
            ORDER BY created_at
            "#,
        )
        .bind(normalize_email(email))
        .bind(format_ts(now()))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| OrgInvitation::try_from(row).ok())
            .collect())
    }

    /// Accept an invitation by token hash. Returns the invitation that was
    /// accepted, or `None` when the token is unknown, used or expired.
    pub async fn accept(&self, token_hash: &str) -> Result<Option<OrgInvitation>> {
        let timestamp = now();
        let row: Option<InvitationRow> = sqlx::query_as(
            r#"
            UPDATE org_invitations
            SET accepted_at = ?2
            WHERE token_hash = ?1 AND accepted_at IS NULL AND expires_at > ?2
            RETURNING id, org_id, email, role, created_by, created_at, expires_at, accepted_at
            "#,
        )
        .bind(token_hash)
        .bind(format_ts(timestamp))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| OrgInvitation::try_from(row).ok()))
    }

    /// Whether a pending invitation exists for an email in an organization.
    pub async fn pending_exists(&self, org_id: &str, email: &str) -> Result<bool> {
        let row: Option<(String,)> = sqlx::query_as(
            r#"
            SELECT id FROM org_invitations
            WHERE org_id = ?1 AND email = ?2 AND accepted_at IS NULL AND expires_at > ?3
            LIMIT 1
            "#,
        )
        .bind(org_id)
        .bind(normalize_email(email))
        .bind(format_ts(now()))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Mark an invitation accepted after the membership was created.
    pub async fn mark_accepted(&self, id: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE org_invitations SET accepted_at = ?2 WHERE id = ?1 AND accepted_at IS NULL",
        )
        .bind(id)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Revoke one invitation.
    pub async fn revoke(&self, id: &str) -> Result<bool> {
        let result =
            sqlx::query("DELETE FROM org_invitations WHERE id = ?1 AND accepted_at IS NULL")
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[derive(Debug, sqlx::FromRow)]
struct InvitationRow {
    id: String,
    org_id: String,
    email: String,
    role: String,
    created_by: String,
    created_at: String,
    expires_at: String,
    accepted_at: Option<String>,
}

impl TryFrom<InvitationRow> for OrgInvitation {
    type Error = ();

    fn try_from(row: InvitationRow) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            org_id: row.org_id,
            email: row.email,
            role: UserRole::parse(&row.role).ok_or(())?,
            created_by: row.created_by,
            created_at: parse_ts(&row.created_at),
            expires_at: parse_ts(&row.expires_at),
            accepted_at: parse_opt_ts(row.accepted_at),
        })
    }
}
