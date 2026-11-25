//! WorkspaceEntries storage operations.

use fleet_core_protocol::workspace::WorkspaceEntry;
use sqlx::{Sqlite, SqlitePool};

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
        replace_in(&mut transaction, workspace_id, parent_path, entries).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Replace the cached entries of several directories in one transaction.
    ///
    /// Watcher flushes touch every parent of a changed batch; committing them
    /// together avoids one write transaction per directory.
    pub async fn replace_many(
        &self,
        workspace_id: &str,
        listings: &[(String, Vec<WorkspaceEntry>)],
    ) -> Result<()> {
        if listings.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for (parent_path, entries) in listings {
            replace_in(&mut transaction, workspace_id, parent_path, entries).await?;
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
        self.delete_under_many(workspace_id, std::slice::from_ref(&path_prefix.to_string()))
            .await
    }

    /// Delete cached entries under several path prefixes in one transaction.
    pub async fn delete_under_many(
        &self,
        workspace_id: &str,
        path_prefixes: &[String],
    ) -> Result<()> {
        if path_prefixes.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for path_prefix in path_prefixes {
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
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
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

/// Replace one directory inside an existing transaction.
async fn replace_in(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    workspace_id: &str,
    parent_path: &str,
    entries: &[WorkspaceEntry],
) -> Result<()> {
    sqlx::query("DELETE FROM workspace_entries WHERE workspace_id = ?1 AND parent_path = ?2")
        .bind(workspace_id)
        .bind(parent_path)
        .execute(&mut **transaction)
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
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}
