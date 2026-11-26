//! Filtering, sorting, faceting and cursor pagination for session lists.
//!
//! Pure functions over merged sessions: the caller resolves ACL visibility
//! first, then hands the page here. Keeping the ordering rules in one place lets
//! the list endpoint, the facet counts and the tests share them.

use fleet_core_protocol::agent::{AgentSession, SessionStatus};

/// Sort orders the session rail offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSort {
    /// Most recently updated first (default).
    Recent,
    /// Newest creation first.
    Created,
    /// Case-insensitive title, then recency.
    Title,
}

impl SessionSort {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "" | "recent" => Some(Self::Recent),
            "created" => Some(Self::Created),
            "title" => Some(Self::Title),
            _ => None,
        }
    }
}

/// A set of filters applied before sorting.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Keep sessions whose status is in this set; empty means every status.
    pub statuses: Vec<SessionStatus>,
    /// `Some(true)` keeps only archived sessions, `Some(false)` only
    /// unarchived ones, `None` ignores the flag.
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
    pub unread: Option<bool>,
    pub machine_id: Option<String>,
    pub workspace_id: Option<String>,
    /// Substring match over title, id, provider, agent type and cwd.
    pub query: Option<String>,
    /// Keep sessions carrying every one of these labels.
    pub labels: Vec<String>,
}

/// Counts shown on the rail's filter chips.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SessionFacets {
    pub all: usize,
    pub running: usize,
    pub waiting: usize,
    pub failed: usize,
    pub closed: usize,
    pub archived: usize,
    pub unread: usize,
    pub pinned: usize,
}

/// One page of sessions plus the next cursor.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionPage {
    pub items: Vec<AgentSession>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Apply `filter` to `sessions`, keeping the original order.
pub fn filter_sessions(sessions: &[AgentSession], filter: &SessionFilter) -> Vec<AgentSession> {
    let query = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(str::to_lowercase);
    let labels: Vec<String> = filter
        .labels
        .iter()
        .map(|label| label.trim().to_lowercase())
        .filter(|label| !label.is_empty())
        .collect();

    sessions
        .iter()
        .filter(|session| {
            if !filter.statuses.is_empty() && !filter.statuses.contains(&session.status) {
                return false;
            }
            let ui = session.ui.as_ref();
            let archived = ui.is_some_and(|ui| ui.archived);
            if filter.archived.is_some_and(|want| want != archived) {
                return false;
            }
            let pinned = ui.is_some_and(|ui| ui.pinned);
            if filter.pinned.is_some_and(|want| want != pinned) {
                return false;
            }
            let unread = ui.is_some_and(|ui| ui.unread);
            if filter.unread.is_some_and(|want| want != unread) {
                return false;
            }
            if filter
                .machine_id
                .as_deref()
                .is_some_and(|machine| machine != session.machine_id)
            {
                return false;
            }
            if filter
                .workspace_id
                .as_deref()
                .is_some_and(|workspace| session.workspace_id.as_deref() != Some(workspace))
            {
                return false;
            }
            if !labels.is_empty() {
                let session_labels: Vec<String> = ui
                    .map(|ui| ui.labels.iter().map(|label| label.to_lowercase()).collect())
                    .unwrap_or_default();
                if labels.iter().any(|label| !session_labels.contains(label)) {
                    return false;
                }
            }
            if let Some(query) = &query {
                let haystack = [
                    session.title.as_deref().unwrap_or_default(),
                    session.id.as_str(),
                    session.provider.as_str(),
                    session.agent_type.as_str(),
                    session.cwd.as_str(),
                ]
                .join("\n")
                .to_lowercase();
                if !haystack.contains(query) {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// Sort the sessions in place.
pub fn sort_sessions(sessions: &mut [AgentSession], sort: SessionSort) {
    sessions.sort_by(|left, right| match sort {
        SessionSort::Recent => right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id)),
        SessionSort::Created => right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id)),
        SessionSort::Title => {
            let left_title = left.title.as_deref().unwrap_or_default().to_lowercase();
            let right_title = right.title.as_deref().unwrap_or_default().to_lowercase();
            left_title
                .cmp(&right_title)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| left.id.cmp(&right.id))
        }
    });
}

/// Count the facet buckets over an already-filtered, ACL-scoped list.
pub fn facets(sessions: &[AgentSession]) -> SessionFacets {
    let mut facets = SessionFacets::default();
    for session in sessions {
        if session.status == SessionStatus::Terminated {
            facets.closed += 1;
        } else {
            facets.all += 1;
        }
        if matches!(
            session.status,
            SessionStatus::Running | SessionStatus::Starting
        ) {
            facets.running += 1;
        }
        if matches!(
            session.status,
            SessionStatus::WaitingForInput
                | SessionStatus::WaitingForPermission
                | SessionStatus::WaitingForQuestion
        ) {
            facets.waiting += 1;
        }
        if session.status == SessionStatus::Failed {
            facets.failed += 1;
        }
        if let Some(ui) = &session.ui {
            if ui.archived {
                facets.archived += 1;
            }
            if ui.unread {
                facets.unread += 1;
            }
            if ui.pinned {
                facets.pinned += 1;
            }
        }
    }
    facets
}

/// Encode the position of one session in a sorted list.
pub fn encode_cursor(session: &AgentSession, sort: SessionSort) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let kind = match sort {
        SessionSort::Recent => "recent",
        SessionSort::Created => "created",
        SessionSort::Title => "title",
    };
    let value = serde_json::json!({
        "s": kind,
        "t": match sort {
            SessionSort::Recent => session.updated_at.to_rfc3339(),
            SessionSort::Created => session.created_at.to_rfc3339(),
            SessionSort::Title => session.title.clone().unwrap_or_default().to_lowercase(),
        },
        "id": session.id,
    });
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap_or_default())
}

/// A decoded list cursor.
#[derive(Debug, Clone)]
struct Cursor {
    sort: String,
    value: String,
    id: String,
}

fn decode_cursor(cursor: &str) -> Option<Cursor> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let bytes = URL_SAFE_NO_PAD.decode(cursor.as_bytes()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(Cursor {
        sort: value.get("s")?.as_str()?.to_string(),
        value: value.get("t")?.as_str()?.to_string(),
        id: value.get("id")?.as_str()?.to_string(),
    })
}

fn sort_kind(sort: SessionSort) -> &'static str {
    match sort {
        SessionSort::Recent => "recent",
        SessionSort::Created => "created",
        SessionSort::Title => "title",
    }
}

/// Sort key of one session as a comparable string.
fn sort_value(session: &AgentSession, sort: SessionSort) -> String {
    match sort {
        SessionSort::Recent => session.updated_at.to_rfc3339(),
        SessionSort::Created => session.created_at.to_rfc3339(),
        SessionSort::Title => session.title.as_deref().unwrap_or_default().to_lowercase(),
    }
}

/// Where a decoded cursor resumes in the sorted list.
///
/// Sorting is descending for recency and ascending for titles, so the
/// comparison direction depends on the sort.
fn cursor_position(sessions: &[AgentSession], sort: SessionSort, cursor: &Cursor) -> usize {
    // Prefer the exact session; it survives ties that a timestamp comparison
    // alone cannot break.
    if let Some(index) = sessions.iter().position(|session| session.id == cursor.id) {
        return index + 1;
    }
    sessions
        .iter()
        .position(|session| {
            let value = sort_value(session, sort);
            match sort {
                SessionSort::Title => value > cursor.value,
                SessionSort::Recent | SessionSort::Created => value < cursor.value,
            }
        })
        .unwrap_or(sessions.len())
}

/// Page a sorted list with an opaque cursor.
///
/// `limit` must already be clamped by the caller.
pub fn paginate(
    sessions: Vec<AgentSession>,
    cursor: Option<&str>,
    limit: usize,
    sort: SessionSort,
) -> SessionPage {
    let start = match cursor.and_then(decode_cursor) {
        Some(cursor) if cursor.sort == sort_kind(sort) => cursor_position(&sessions, sort, &cursor),
        // A cursor from another sort order is meaningless; start over rather
        // than return an arbitrary window.
        _ => 0,
    };
    if start >= sessions.len() {
        return SessionPage {
            items: Vec::new(),
            next_cursor: None,
        };
    }
    let end = (start + limit).min(sessions.len());
    let next_cursor = (end < sessions.len()).then(|| encode_cursor(&sessions[end - 1], sort));
    SessionPage {
        items: sessions[start..end].to_vec(),
        next_cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core_common::now;
    use fleet_core_protocol::agent::SessionUiState;

    fn session(id: &str, status: SessionStatus, minutes_ago: i64) -> AgentSession {
        let mut session = AgentSession::new("m_1", "claude", "claude-code", "/repo");
        session.id = id.to_string();
        session.title = Some(id.to_uppercase());
        session.status = status;
        session.created_at = now() - chrono::Duration::minutes(minutes_ago);
        session.updated_at = session.created_at;
        session
    }

    fn with_ui(
        mut session: AgentSession,
        pinned: bool,
        archived: bool,
        unread: bool,
    ) -> AgentSession {
        session.ui = Some(SessionUiState {
            pinned,
            archived,
            unread,
            ..SessionUiState::default()
        });
        session
    }

    #[test]
    fn filters_by_status_flags_and_query() {
        let sessions = vec![
            with_ui(session("a", SessionStatus::Running, 1), false, false, true),
            with_ui(
                session("b", SessionStatus::Terminated, 2),
                false,
                true,
                false,
            ),
            with_ui(session("c", SessionStatus::Failed, 3), true, false, false),
        ];

        let running = filter_sessions(
            &sessions,
            &SessionFilter {
                statuses: vec![SessionStatus::Running],
                ..SessionFilter::default()
            },
        );
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, "a");

        let archived = filter_sessions(
            &sessions,
            &SessionFilter {
                archived: Some(true),
                ..SessionFilter::default()
            },
        );
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "b");

        let unread = filter_sessions(
            &sessions,
            &SessionFilter {
                unread: Some(true),
                ..SessionFilter::default()
            },
        );
        assert_eq!(unread.len(), 1);

        let pinned = filter_sessions(
            &sessions,
            &SessionFilter {
                pinned: Some(true),
                ..SessionFilter::default()
            },
        );
        assert_eq!(pinned[0].id, "c");

        let by_query = filter_sessions(
            &sessions,
            &SessionFilter {
                query: Some("claude-code".to_string()),
                ..SessionFilter::default()
            },
        );
        assert_eq!(by_query.len(), 3);
    }

    #[test]
    fn sorts_recent_created_and_title() {
        let mut sessions = vec![
            session("old", SessionStatus::Completed, 30),
            session("new", SessionStatus::Completed, 1),
        ];
        sort_sessions(&mut sessions, SessionSort::Recent);
        assert_eq!(sessions[0].id, "new");

        sessions[0].title = Some("zeta".into());
        sessions[1].title = Some("alpha".into());
        sort_sessions(&mut sessions, SessionSort::Title);
        assert_eq!(sessions[0].title.as_deref(), Some("alpha"));
    }

    #[test]
    fn facets_count_each_bucket() {
        let sessions = vec![
            with_ui(session("a", SessionStatus::Running, 1), true, false, true),
            with_ui(
                session("b", SessionStatus::WaitingForPermission, 2),
                false,
                false,
                false,
            ),
            session("c", SessionStatus::Failed, 3),
            with_ui(
                session("d", SessionStatus::Terminated, 4),
                false,
                true,
                false,
            ),
        ];
        let facets = facets(&sessions);
        assert_eq!(facets.all, 3);
        assert_eq!(facets.running, 1);
        assert_eq!(facets.waiting, 1);
        assert_eq!(facets.failed, 1);
        assert_eq!(facets.closed, 1);
        assert_eq!(facets.archived, 1);
        assert_eq!(facets.unread, 1);
        assert_eq!(facets.pinned, 1);
    }

    #[test]
    fn pagination_walks_every_item_once() {
        let sessions: Vec<AgentSession> = (0..5)
            .map(|index| session(&format!("s{index}"), SessionStatus::Completed, index))
            .collect();
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = paginate(sessions.clone(), cursor.as_deref(), 2, SessionSort::Recent);
            assert!(page.items.len() <= 2);
            all.extend(page.items.iter().map(|session| session.id.clone()));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(all, vec!["s0", "s1", "s2", "s3", "s4"]);
    }

    #[test]
    fn a_cursor_from_another_sort_restarts() {
        let sessions: Vec<AgentSession> = (0..3)
            .map(|index| session(&format!("s{index}"), SessionStatus::Completed, index))
            .collect();
        let cursor = encode_cursor(&sessions[0], SessionSort::Recent);
        let page = paginate(sessions, Some(&cursor), 2, SessionSort::Title);
        assert_eq!(page.items[0].id, "s0");
    }
}
