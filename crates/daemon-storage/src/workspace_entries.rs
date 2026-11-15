//! WorkspaceEntries storage operations.

use fleet_core_protocol::workspace::WorkspaceEntry;
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

use crate::rows::EntryRow;

/// Handle for workspace_entries queries.
#[derive(Clone)]
pub struct WorkspaceEntries {
    pool: SqlitePool,
}

impl WorkspaceEntries {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl WorkspaceEntries {
    /// Replace all cached entries for one directory in a workspace.
    ///
    /// The delete and inserts run in a single transaction so readers never see
    /// a half-replaced directory.
    pub async fn replace(
        &self,
        workspace_id: &str,
        parent_path: &str,
        entries: &[WorkspaceEntry],
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query("DELETE FROM workspace_entries WHERE workspace_id = ?1 AND parent_path = ?2")
            .bind(workspace_id)
            .bind(parent_path)
            .execute(&mut *transaction)
            .await?;

        for entry in entries {
            sqlx::query(
                r#"
                INSERT INTO workspace_entries (
                    workspace_id, parent_path, path, name, entry_type,
                    size, mtime_ns, git_status, ignored
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(workspace_id, path) DO UPDATE SET
                    parent_path = excluded.parent_path,
                    name = excluded.name,
                    entry_type = excluded.entry_type,
                    size = excluded.size,
                    mtime_ns = excluded.mtime_ns,
                    git_status = excluded.git_status,
                    ignored = excluded.ignored
                "#,
            )
            .bind(workspace_id)
            .bind(parent_path)
            .bind(&entry.path)
            .bind(&entry.name)
            .bind(enum_text(&entry.entry_type))
            .bind(entry.size.and_then(|size| i64::try_from(size).ok()))
            .bind(entry.modified_at.map(ts_to_nanos))
            .bind(entry.git_status.as_ref().map(enum_text))
            .bind(bool_to_int(entry.ignored))
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(())
    }

    /// List cached entries for one directory, ordered by path.
    pub async fn list(&self, workspace_id: &str, parent_path: &str) -> Result<Vec<WorkspaceEntry>> {
        let rows = sqlx::query_as::<_, EntryRow>(
            r#"
            SELECT path, name, entry_type, size, mtime_ns, git_status, ignored
            FROM workspace_entries
            WHERE workspace_id = ?1 AND parent_path = ?2
            ORDER BY path ASC
            "#,
        )
        .bind(workspace_id)
        .bind(parent_path)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(EntryRow::into_entry).collect())
    }

    /// Delete every cached entry at or below `path_prefix` in a workspace.
    ///
    /// A row matches when its path equals the prefix or starts with
    /// `prefix + '/'`. LIKE wildcards inside the prefix are escaped.
    pub async fn delete_under(&self, workspace_id: &str, path_prefix: &str) -> Result<()> {
        let pattern = format!("{}/%", escape_like(path_prefix));

        sqlx::query(
            r#"
            DELETE FROM workspace_entries
            WHERE workspace_id = ?1
              AND (path = ?2 OR path LIKE ?3 ESCAPE '\')
            "#,
        )
        .bind(workspace_id)
        .bind(path_prefix)
        .bind(pattern)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Delete every cached directory entry for a workspace.
    pub async fn delete_all(&self, workspace_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM workspace_entries WHERE workspace_id = ?1")
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
