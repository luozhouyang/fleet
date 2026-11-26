//! Per-user session presentation state: overlay, unread derivation and patches.
//!
//! The admin stores no `ui` field on the session row; every response that
//! carries a session attaches the calling actor's state here. Unread is derived
//! from the session's agent-activity watermark against the actor's read
//! watermark, so it needs no per-event fan-out and survives restarts.

use std::collections::{HashMap, HashSet};

use fleet_admin_storage::session_ui::SessionUiPatch;
use fleet_core_common::Timestamp;
use fleet_core_protocol::agent::{normalize_session_labels, AgentSession, UpdateSessionUiRequest};

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::state::AppState;

/// The storage key for the calling principal.
///
/// The bootstrap token has no user row, so it shares one fixed actor id.
pub fn actor_id(principal: &Principal) -> String {
    match principal.user() {
        Some(user) => user.id.clone(),
        None => fleet_admin_storage::session_ui::LEGACY_ACTOR.to_string(),
    }
}

/// Attach each session's presentation state for `actor`.
///
/// Two batched queries cover the whole page regardless of its size. A session
/// without stored state gets the default state with `unread` computed anyway,
/// so a session that has never been opened still shows activity.
pub async fn overlay(state: &AppState, actor: &str, sessions: &mut [AgentSession]) {
    if sessions.is_empty() {
        return;
    }
    let ids: Vec<String> = sessions.iter().map(|session| session.id.clone()).collect();

    let mut ui = match state.db.session_ui.get_many(actor, &ids).await {
        Ok(ui) => ui,
        Err(error) => {
            tracing::warn!(actor, error = %error, "failed to load session UI state");
            HashMap::new()
        }
    };
    let reads = match state.db.session_ui.read_watermarks(actor, &ids).await {
        Ok(reads) => reads,
        Err(error) => {
            tracing::warn!(actor, error = %error, "failed to load session read markers");
            HashMap::new()
        }
    };
    let activity = match state.db.sessions.activity_watermarks(&ids).await {
        Ok(activity) => activity,
        Err(error) => {
            tracing::warn!(actor, error = %error, "failed to load session activity");
            HashMap::new()
        }
    };

    for session in sessions.iter_mut() {
        let mut session_ui = ui.remove(&session.id).unwrap_or_default();
        session_ui.unread = unread_for(
            activity.get(&session.id),
            reads.get(&session.id),
            &session.status,
        );
        session.ui = Some(session_ui);
    }
}

/// Whether activity happened after the actor's last read.
///
/// `None` activity means nothing agent-generated has happened yet. Terminal
/// sessions never count as unread: there is no live work to come back to.
fn unread_for(
    activity: Option<&String>,
    read: Option<&String>,
    status: &fleet_core_protocol::agent::SessionStatus,
) -> bool {
    let Some(activity) = activity else {
        return false;
    };
    if matches!(
        status,
        fleet_core_protocol::agent::SessionStatus::Terminated
            | fleet_core_protocol::agent::SessionStatus::Completed
    ) {
        return false;
    }
    match read {
        Some(read) => activity > read,
        None => true,
    }
}

/// Mark one session unread for the caller.
pub async fn mark_unread(state: &AppState, actor: &str, session_id: &str) -> Result<(), AppError> {
    state
        .db
        .session_ui
        .mark_unread(actor, session_id)
        .await
        .map_err(AppError::internal)
}

/// Reject folder ids the actor does not own.
async fn validate_folder(state: &AppState, actor: &str, folder_id: &str) -> Result<(), AppError> {
    let folders = state
        .db
        .session_ui
        .list_folders(actor)
        .await
        .map_err(AppError::internal)?;
    if folders.iter().any(|folder| folder.id == folder_id) {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "folder does not belong to the caller",
        ))
    }
}

/// Translate a request into a storage patch after validating labels and folder.
pub async fn to_patch(
    state: &AppState,
    actor: &str,
    request: &UpdateSessionUiRequest,
) -> Result<SessionUiPatch, AppError> {
    let labels = match &request.labels {
        Some(labels) => Some(normalize_session_labels(labels.clone()).map_err(AppError::from)?),
        None => None,
    };
    if let Some(Some(folder_id)) = &request.folder_id {
        validate_folder(state, actor, folder_id).await?;
    }
    Ok(SessionUiPatch {
        pinned: request.pinned,
        archived: request.archived,
        labels,
        folder_id: request.folder_id.clone(),
    })
}

/// Apply a patch to one session and return the updated session with its UI
/// state attached.
pub async fn patch_session(
    state: &AppState,
    actor: &str,
    session: AgentSession,
    patch: &SessionUiPatch,
) -> Result<AgentSession, AppError> {
    state
        .db
        .session_ui
        .patch(actor, &session.id, patch)
        .await
        .map_err(AppError::internal)?;
    let mut sessions = vec![session];
    overlay(state, actor, &mut sessions).await;
    Ok(sessions.pop().expect("one session"))
}

/// Apply one patch to several sessions, skipping duplicates.
pub async fn patch_many(
    state: &AppState,
    actor: &str,
    session_ids: &[String],
    patch: &SessionUiPatch,
) -> Result<(), AppError> {
    let mut seen = HashSet::new();
    let ids: Vec<String> = session_ids
        .iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect();
    state
        .db
        .session_ui
        .patch_many(actor, &ids, patch)
        .await
        .map_err(AppError::internal)
}

/// Record a read watermark for one or more sessions.
///
/// `watermark` is the `updated_at` the client observed; a stale value can only
/// leave a session read, never mark newer activity read, because storage keeps
/// the maximum.
pub async fn mark_read(
    state: &AppState,
    actor: &str,
    session_ids: &[String],
    watermark: Timestamp,
) -> Result<(), AppError> {
    state
        .db
        .session_ui
        .mark_read(actor, session_ids, watermark)
        .await
        .map_err(AppError::internal)
}

/// Clamp a requested page size into the accepted range.
pub fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Maximum sessions in one bulk UI update or pin reorder.
pub const MAX_BULK_IDS: usize = 500;

/// Default page size for the session rail.
pub const DEFAULT_LIMIT: usize = 500;
/// Hard upper bound for one page.
pub const MAX_LIMIT: usize = 2000;

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core_protocol::agent::SessionStatus;

    #[test]
    fn unread_requires_activity_after_the_read_watermark() {
        let activity = "2026-01-01T10:00:00+00:00".to_string();
        assert!(unread_for(Some(&activity), None, &SessionStatus::Running));
        assert!(!unread_for(
            Some(&activity),
            Some(&"2026-01-01T10:00:00+00:00".to_string()),
            &SessionStatus::Running
        ));
        assert!(unread_for(
            Some(&activity),
            Some(&"2026-01-01T09:00:00+00:00".to_string()),
            &SessionStatus::Running
        ));
        // Nothing agent-generated yet.
        assert!(!unread_for(None, None, &SessionStatus::Running));
    }

    #[test]
    fn terminal_sessions_are_never_unread() {
        let activity = "2026-01-01T10:00:00+00:00".to_string();
        assert!(!unread_for(
            Some(&activity),
            None,
            &SessionStatus::Completed
        ));
        assert!(!unread_for(
            Some(&activity),
            None,
            &SessionStatus::Terminated
        ));
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(10_000)), MAX_LIMIT);
    }
}
