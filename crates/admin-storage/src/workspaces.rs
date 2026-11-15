//! Workspaces storage operations.

use fleet_core_protocol::workspace::Workspace;
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::rows::WorkspaceRow;
use crate::Result;

/// Handle for workspaces queries.
#[derive(Clone)]
pub struct Workspaces {
    pool: SqlitePool,
}

impl Workspaces {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Workspaces {
    /// Insert a workspace or replace its mutable fields.
    pub async fn upsert(&self, w: &Workspace) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO workspaces (
                id, machine_id, project_id, name, root_path,
                is_git_repository, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(id) DO UPDATE SET
                machine_id = excluded.machine_id,
                project_id = excluded.project_id,
                name = excluded.name,
                root_path = excluded.root_path,
                is_git_repository = excluded.is_git_repository,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&w.id)
        .bind(&w.machine_id)
        .bind(&w.project_id)
        .bind(&w.name)
        .bind(&w.root_path)
        .bind(bool_to_int(w.is_git_repository))
        .bind(format_ts(w.created_at))
        .bind(format_ts(w.updated_at))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch one workspace by id.
    pub async fn get(&self, id: &str) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            r#"
            SELECT id, machine_id, project_id, name, root_path,
                   is_git_repository, created_at, updated_at
            FROM workspaces
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(WorkspaceRow::into_workspace))
    }

    /// List workspaces, optionally restricted to one machine, ordered by name.
    pub async fn list(&self, machine_id: Option<&str>) -> Result<Vec<Workspace>> {
        let rows = match machine_id {
            Some(machine_id) => {
                sqlx::query_as::<_, WorkspaceRow>(
                    r#"
                    SELECT id, machine_id, project_id, name, root_path,
                           is_git_repository, created_at, updated_at
                    FROM workspaces
                    WHERE machine_id = ?1
                    ORDER BY name ASC, id ASC
                    "#,
                )
                .bind(machine_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, WorkspaceRow>(
                    r#"
                    SELECT id, machine_id, project_id, name, root_path,
                           is_git_repository, created_at, updated_at
                    FROM workspaces
                    ORDER BY name ASC, id ASC
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows.into_iter().map(WorkspaceRow::into_workspace).collect())
    }

    /// Delete a workspace row.
    pub async fn delete(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM workspaces WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
