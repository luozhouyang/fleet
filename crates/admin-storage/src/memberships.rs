//! Membership storage: which user has which role in which organization.

use fleet_core_common::now;
use fleet_core_protocol::authz::{Membership, OrgMember, UserRole};
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

/// Handle for membership queries.
#[derive(Clone)]
pub struct Memberships {
    pool: SqlitePool,
}

impl Memberships {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Memberships {
    /// Insert a membership, or update the role when it already exists.
    pub async fn upsert(
        &self,
        org_id: &str,
        user_id: &str,
        role: UserRole,
        invited_by: Option<&str>,
    ) -> Result<Membership> {
        let timestamp = now();
        sqlx::query(
            r#"
            INSERT INTO memberships (org_id, user_id, role, created_at, updated_at, invited_by)
            VALUES (?1, ?2, ?3, ?4, ?4, ?5)
            ON CONFLICT(org_id, user_id) DO UPDATE SET
                role = excluded.role,
                updated_at = excluded.updated_at,
                invited_by = COALESCE(excluded.invited_by, memberships.invited_by)
            "#,
        )
        .bind(org_id)
        .bind(user_id)
        .bind(role.as_str())
        .bind(format_ts(timestamp))
        .bind(invited_by)
        .execute(&self.pool)
        .await?;
        self.get(org_id, user_id).await?.ok_or_else(|| {
            crate::StorageError::InvalidData("membership vanished after upsert".into())
        })
    }

    /// Fetch one membership.
    pub async fn get(&self, org_id: &str, user_id: &str) -> Result<Option<Membership>> {
        let row: Option<MembershipRow> = sqlx::query_as(
            r#"
            SELECT org_id, user_id, role, created_at, updated_at, invited_by
            FROM memberships WHERE org_id = ?1 AND user_id = ?2
            "#,
        )
        .bind(org_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| Membership::try_from(row).ok()))
    }

    /// Every membership of a user in a live organization, oldest first.
    ///
    /// Soft delete removes memberships in the same transaction, but a write
    /// racing the delete could re-insert one; joining the organization keeps
    /// such a stale row from granting access.
    pub async fn for_user(&self, user_id: &str) -> Result<Vec<Membership>> {
        let rows: Vec<MembershipRow> = sqlx::query_as(
            r#"
            SELECT m.org_id, m.user_id, m.role, m.created_at, m.updated_at,
                   m.invited_by
            FROM memberships m
            JOIN organizations o ON o.id = m.org_id
            WHERE m.user_id = ?1 AND o.deleted_at IS NULL
            ORDER BY m.created_at
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| Membership::try_from(row).ok())
            .collect())
    }

    /// Every membership of an organization, joined with the member profile.
    pub async fn members_of(&self, org_id: &str) -> Result<Vec<OrgMember>> {
        let rows: Vec<OrgMemberRow> = sqlx::query_as(
            r#"
            SELECT m.user_id AS user_id, u.email AS email, u.name AS name,
                   u.avatar_url AS avatar_url, m.role AS role,
                   m.created_at AS created_at, m.updated_at AS updated_at
            FROM memberships m
            JOIN users u ON u.id = m.user_id
            WHERE m.org_id = ?1
            ORDER BY m.created_at
            "#,
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| OrgMember::try_from(row).ok())
            .collect())
    }

    /// Remove a membership. Returns false when it did not exist.
    pub async fn remove(&self, org_id: &str, user_id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM memberships WHERE org_id = ?1 AND user_id = ?2")
            .bind(org_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Remove every membership of one user. Used when an account is deleted.
    pub async fn remove_all_for_user(&self, user_id: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM memberships WHERE user_id = ?1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Number of members holding one role.
    pub async fn count_role(&self, org_id: &str, role: UserRole) -> Result<i64> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM memberships WHERE org_id = ?1 AND role = ?2")
                .bind(org_id)
                .bind(role.as_str())
                .fetch_one(&self.pool)
                .await?;
        Ok(row.0)
    }

    /// Number of members in an organization.
    pub async fn count(&self, org_id: &str) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memberships WHERE org_id = ?1")
            .bind(org_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }
}

#[derive(Debug, sqlx::FromRow)]
struct MembershipRow {
    org_id: String,
    user_id: String,
    role: String,
    created_at: String,
    updated_at: String,
    invited_by: Option<String>,
}

impl TryFrom<MembershipRow> for Membership {
    type Error = ();

    fn try_from(row: MembershipRow) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            org_id: row.org_id,
            user_id: row.user_id,
            role: UserRole::parse(&row.role).ok_or(())?,
            created_at: parse_ts(&row.created_at),
            updated_at: parse_ts(&row.updated_at),
            invited_by: row.invited_by,
        })
    }
}

#[derive(Debug, sqlx::FromRow)]
struct OrgMemberRow {
    user_id: String,
    email: Option<String>,
    name: String,
    avatar_url: Option<String>,
    role: String,
    created_at: String,
    updated_at: String,
}

impl TryFrom<OrgMemberRow> for OrgMember {
    type Error = ();

    fn try_from(row: OrgMemberRow) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            user_id: row.user_id,
            email: row.email,
            name: row.name,
            avatar_url: row.avatar_url,
            role: UserRole::parse(&row.role).ok_or(())?,
            created_at: parse_ts(&row.created_at),
            updated_at: parse_ts(&row.updated_at),
        })
    }
}
