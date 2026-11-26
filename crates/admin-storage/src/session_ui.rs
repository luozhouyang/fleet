//! Per-user session presentation state: pins, archive, labels, folders, reads.
//!
//! Everything here is keyed by `actor_id` — the authenticated user id, or the
//! [`LEGACY_ACTOR`] sentinel for the bootstrap admin token. The daemon knows
//! nothing about these rows, so they survive daemon restarts and session
//! reconciliation.

use std::collections::HashMap;

use fleet_core_common::now;
use fleet_core_protocol::agent::SessionUiState;
use sqlx::SqlitePool;

use crate::helpers::*;
use crate::Result;

/// Actor id used for the bootstrap admin token, which has no user row.
pub const LEGACY_ACTOR: &str = "__legacy__";

/// Maximum folders per actor.
pub const MAX_FOLDERS_PER_ACTOR: usize = 100;
/// Maximum characters in a folder name.
pub const MAX_FOLDER_NAME_CHARS: usize = 64;

/// One folder belonging to an actor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SessionFolder {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
}

/// A partial update of one session's presentation state.
///
/// `None` leaves the stored value untouched. `labels` replaces the whole list.
/// `folder_id` is doubly optional: the outer `None` leaves it alone, and
/// `Some(None)` removes the session from its folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionUiPatch {
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub labels: Option<Vec<String>>,
    pub folder_id: Option<Option<String>>,
}

impl SessionUiPatch {
    pub fn is_empty(&self) -> bool {
        self.pinned.is_none()
            && self.archived.is_none()
            && self.labels.is_none()
            && self.folder_id.is_none()
    }
}

/// Folder membership for one actor.
#[derive(Debug, Clone)]
pub struct SessionUi {
    pool: SqlitePool,
}

impl SessionUi {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl SessionUi {
    /// Load the presentation state of several sessions for one actor.
    ///
    /// Sessions without a row are absent from the map; callers treat them as
    /// the default state.
    pub async fn get_many(
        &self,
        actor_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, SessionUiState>> {
        let mut states = HashMap::new();
        if session_ids.is_empty() {
            return Ok(states);
        }

        let state_placeholders = placeholders(session_ids.len(), 1);
        let sql = format!(
            r#"
            SELECT session_id, pinned, pinned_rank, archived, labels_json
            FROM session_ui_state
            WHERE actor_id = ?1 AND session_id IN ({state_placeholders})
            "#
        );
        let mut query = sqlx::query_as::<_, SessionUiRow>(&sql).bind(actor_id);
        for id in session_ids {
            query = query.bind(id);
        }
        let folders: HashMap<String, String> = {
            let folder_placeholders = placeholders(session_ids.len(), 1);
            let sql = format!(
                r#"
                SELECT session_id, folder_id FROM session_folder_members
                WHERE actor_id = ?1 AND session_id IN ({folder_placeholders})
                "#
            );
            let mut query = sqlx::query_as::<_, (String, String)>(&sql).bind(actor_id);
            for id in session_ids {
                query = query.bind(id);
            }
            query.fetch_all(&self.pool).await?.into_iter().collect()
        };

        for row in query.fetch_all(&self.pool).await? {
            states.insert(
                row.session_id.clone(),
                SessionUiState {
                    pinned: row.pinned != 0,
                    pinned_rank: row.pinned_rank,
                    archived: row.archived != 0,
                    unread: false,
                    labels: decode_json_or_default(&row.labels_json),
                    folder_id: folders.get(&row.session_id).cloned(),
                },
            );
        }
        // A folder membership without a UI row still surfaces.
        for (session_id, folder_id) in folders {
            states.entry(session_id).or_insert_with(|| SessionUiState {
                folder_id: Some(folder_id),
                ..SessionUiState::default()
            });
        }
        Ok(states)
    }

    /// Apply a patch to one session, inserting the row when needed.
    ///
    /// Returns `false` when the session does not exist.
    pub async fn patch(
        &self,
        actor_id: &str,
        session_id: &str,
        patch: &SessionUiPatch,
    ) -> Result<bool> {
        if patch.is_empty() {
            return self.session_exists(session_id).await;
        }
        // Take the write lock up front: the follow-up statements depend on the
        // row this transaction creates, and a deferred transaction could fail
        // to upgrade if a daemon reconciliation commits in between.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !session_exists_on(&mut tx, session_id).await? {
            tx.rollback().await?;
            return Ok(false);
        }

        sqlx::query(
            r#"
            INSERT INTO session_ui_state
                (actor_id, session_id, pinned, pinned_rank, archived, labels_json, updated_at)
            VALUES (?1, ?2, 0, NULL, 0, '[]', ?3)
            ON CONFLICT(actor_id, session_id) DO NOTHING
            "#,
        )
        .bind(actor_id)
        .bind(session_id)
        .bind(format_ts(now()))
        .execute(&mut *tx)
        .await?;

        if let Some(pinned) = patch.pinned {
            // A newly pinned session joins the end of the pinned group.
            sqlx::query(
                r#"
                UPDATE session_ui_state
                SET pinned = ?3,
                    pinned_rank = CASE WHEN ?3 = 1 THEN COALESCE(pinned_rank,
                        (SELECT COALESCE(MAX(pinned_rank), 0) + 1
                         FROM session_ui_state WHERE actor_id = ?1)) ELSE NULL END,
                    updated_at = ?4
                WHERE actor_id = ?1 AND session_id = ?2
                "#,
            )
            .bind(actor_id)
            .bind(session_id)
            .bind(bool_to_int(pinned))
            .bind(format_ts(now()))
            .execute(&mut *tx)
            .await?;
        }
        if let Some(archived) = patch.archived {
            sqlx::query(
                "UPDATE session_ui_state SET archived = ?3, updated_at = ?4 \
                 WHERE actor_id = ?1 AND session_id = ?2",
            )
            .bind(actor_id)
            .bind(session_id)
            .bind(bool_to_int(archived))
            .bind(format_ts(now()))
            .execute(&mut *tx)
            .await?;
        }
        if let Some(labels) = &patch.labels {
            sqlx::query(
                "UPDATE session_ui_state SET labels_json = ?3, updated_at = ?4 \
                 WHERE actor_id = ?1 AND session_id = ?2",
            )
            .bind(actor_id)
            .bind(session_id)
            .bind(serde_json::to_string(labels)?)
            .bind(format_ts(now()))
            .execute(&mut *tx)
            .await?;
        }
        if let Some(folder_id) = &patch.folder_id {
            sqlx::query(
                "DELETE FROM session_folder_members WHERE actor_id = ?1 AND session_id = ?2",
            )
            .bind(actor_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
            if let Some(folder_id) = folder_id {
                sqlx::query(
                    "INSERT INTO session_folder_members (actor_id, folder_id, session_id) \
                     VALUES (?1, ?2, ?3)",
                )
                .bind(actor_id)
                .bind(folder_id)
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(true)
    }

    /// Apply one patch to several sessions in a single transaction.
    ///
    /// Sessions the actor cannot see are skipped by the caller before this
    /// runs; an unknown id simply inserts nothing.
    pub async fn patch_many(
        &self,
        actor_id: &str,
        session_ids: &[String],
        patch: &SessionUiPatch,
    ) -> Result<()> {
        if patch.is_empty() || session_ids.is_empty() {
            return Ok(());
        }
        for session_id in session_ids {
            self.patch(actor_id, session_id, patch).await?;
        }
        Ok(())
    }

    /// Replace the pinned order with `session_ids`, which must all be pinned.
    ///
    /// The list is stored as ranks starting at 1; unlisted pinned sessions keep
    /// their existing rank and sort afterwards.
    pub async fn replace_pins(&self, actor_id: &str, session_ids: &[String]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        // Offset the new ranks so they always sort before stale ones, then
        // renumber the requested ids from 1.
        for (index, session_id) in session_ids.iter().enumerate() {
            sqlx::query(
                "UPDATE session_ui_state SET pinned = 1, pinned_rank = ?3, updated_at = ?4 \
                 WHERE actor_id = ?1 AND session_id = ?2",
            )
            .bind(actor_id)
            .bind(session_id)
            .bind(index as i64 + 1)
            .bind(format_ts(now()))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Record that `actor_id` has read the sessions as of `watermark`.
    ///
    /// The stored watermark only moves forward, so a stale client cannot mark
    /// newer activity read.
    pub async fn mark_read(
        &self,
        actor_id: &str,
        session_ids: &[String],
        watermark: fleet_core_common::Timestamp,
    ) -> Result<()> {
        if session_ids.is_empty() {
            return Ok(());
        }
        let watermark = format_ts(watermark);
        let mut tx = self.pool.begin().await?;
        for session_id in session_ids {
            sqlx::query(
                r#"
                INSERT INTO session_reads (actor_id, session_id, last_read_at)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(actor_id, session_id) DO UPDATE SET
                    last_read_at = MAX(last_read_at, excluded.last_read_at)
                "#,
            )
            .bind(actor_id)
            .bind(session_id)
            .bind(&watermark)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Load read watermarks for one actor, keyed by session id.
    pub async fn read_watermarks(
        &self,
        actor_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, String>> {
        let mut watermarks = HashMap::new();
        if session_ids.is_empty() {
            return Ok(watermarks);
        }
        let placeholders = placeholders(session_ids.len(), 1);
        let sql = format!(
            "SELECT session_id, last_read_at FROM session_reads \
             WHERE actor_id = ?1 AND session_id IN ({placeholders})"
        );
        let mut query = sqlx::query_as::<_, (String, String)>(&sql).bind(actor_id);
        for id in session_ids {
            query = query.bind(id);
        }
        for (session_id, last_read_at) in query.fetch_all(&self.pool).await? {
            watermarks.insert(session_id, last_read_at);
        }
        Ok(watermarks)
    }

    /// List the folders of one actor, in display order.
    pub async fn list_folders(&self, actor_id: &str) -> Result<Vec<SessionFolder>> {
        let rows = sqlx::query_as::<_, (String, String, i64)>(
            "SELECT id, name, sort_order FROM session_folders \
             WHERE actor_id = ?1 ORDER BY sort_order ASC, name ASC",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, sort_order)| SessionFolder {
                id,
                name,
                sort_order,
            })
            .collect())
    }

    /// Create a folder for one actor.
    pub async fn create_folder(&self, actor_id: &str, name: &str) -> Result<SessionFolder> {
        let name = name.trim();
        if name.is_empty() {
            return Err(crate::StorageError::InvalidData(
                "folder name must not be empty".to_string(),
            ));
        }
        if name.chars().count() > MAX_FOLDER_NAME_CHARS {
            return Err(crate::StorageError::InvalidData(format!(
                "folder name is longer than {MAX_FOLDER_NAME_CHARS} characters"
            )));
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM session_folders WHERE actor_id = ?1")
                .bind(actor_id)
                .fetch_one(&self.pool)
                .await?;
        if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_FOLDERS_PER_ACTOR {
            return Err(crate::StorageError::InvalidData(format!(
                "at most {MAX_FOLDERS_PER_ACTOR} folders are allowed"
            )));
        }
        let sort_order: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM session_folders WHERE actor_id = ?1",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;
        let id = fleet_core_common::new_id("folder");
        let timestamp = format_ts(now());
        sqlx::query(
            "INSERT INTO session_folders (id, actor_id, name, sort_order, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        )
        .bind(&id)
        .bind(actor_id)
        .bind(name)
        .bind(sort_order)
        .bind(&timestamp)
        .execute(&self.pool)
        .await?;
        Ok(SessionFolder {
            id,
            name: name.to_string(),
            sort_order,
        })
    }

    /// Rename a folder the actor owns. Returns `false` when it does not exist.
    pub async fn rename_folder(&self, actor_id: &str, folder_id: &str, name: &str) -> Result<bool> {
        let name = name.trim();
        if name.is_empty() {
            return Err(crate::StorageError::InvalidData(
                "folder name must not be empty".to_string(),
            ));
        }
        if name.chars().count() > MAX_FOLDER_NAME_CHARS {
            return Err(crate::StorageError::InvalidData(format!(
                "folder name is longer than {MAX_FOLDER_NAME_CHARS} characters"
            )));
        }
        let result = sqlx::query(
            "UPDATE session_folders SET name = ?3, updated_at = ?4 \
             WHERE actor_id = ?1 AND id = ?2",
        )
        .bind(actor_id)
        .bind(folder_id)
        .bind(name)
        .bind(format_ts(now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete a folder and its memberships. Returns `false` when absent.
    pub async fn delete_folder(&self, actor_id: &str, folder_id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM session_folders WHERE actor_id = ?1 AND id = ?2")
            .bind(actor_id)
            .bind(folder_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn session_exists(&self, session_id: &str) -> Result<bool> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM agent_sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(exists.is_some())
    }
}

#[derive(sqlx::FromRow)]
struct SessionUiRow {
    session_id: String,
    pinned: i64,
    pinned_rank: Option<i64>,
    archived: i64,
    labels_json: String,
}

/// Build `?2, ?3, ...` placeholders starting after `offset` bound parameters.
fn placeholders(count: usize, offset: usize) -> String {
    (1..=count)
        .map(|index| format!("?{}", index + offset))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn session_exists_on(tx: &mut sqlx::SqliteConnection, session_id: &str) -> Result<bool> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM agent_sessions WHERE id = ?1")
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;
    Ok(exists.is_some())
}
