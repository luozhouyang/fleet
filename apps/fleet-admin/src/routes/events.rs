//! Browser event WebSocket at `/api/v1/events`.
//!
//! Browsers cannot attach an `Authorization` header to a WebSocket upgrade, so
//! this endpoint accepts either that header or a `?token=` query parameter.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use fleet_admin_eventbus::EventFilter;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::error::AppError;
use crate::middleware::auth::{resolve_principal, Principal};
use crate::state::AppState;

/// Query parameters accepted by the event socket.
#[derive(Debug, Deserialize, Default)]
pub struct EventAuthQuery {
    /// Admin token for browser clients that cannot set headers.
    #[serde(default)]
    pub token: Option<String>,
}

/// `GET /api/v1/events` (WebSocket upgrade).
pub async fn events_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventAuthQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let credentials = bearer_credentials(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    )
    .or(query.token.as_deref())
    .ok_or_else(|| {
        AppError::unauthorized("missing or invalid bearer token or token query parameter")
    })?;
    let principal = resolve_principal(&state, credentials).await?;
    let credentials = credentials.to_string();

    Ok(ws.on_upgrade(move |socket| stream_events(socket, state, principal, credentials)))
}

fn bearer_credentials(header: Option<&str>) -> Option<&str> {
    let header = header?.trim();
    let (scheme, credentials) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let credentials = credentials.trim();
    (!credentials.is_empty()).then_some(credentials)
}

async fn stream_events(
    socket: WebSocket,
    state: AppState,
    principal: Principal,
    credentials: String,
) {
    let filter = EventFilter::new();
    let mut events = state.events.subscribe();
    // Re-authenticate periodically so deleted users and rotated tokens stop
    // receiving events even if they keep an old WebSocket open.
    let mut reauth = tokio::time::interval(std::time::Duration::from_secs(15));
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(25));
    let (mut sink, mut stream) = socket.split();

    loop {
        tokio::select! {
            received = events.recv() => {
                match received {
                    Ok(event) => {
                        if !filter.matches(&event) {
                            continue;
                        }
                        if let Some(machine_id) = event.machine_id.as_deref() {
                            if !event_visible(&state, &principal, machine_id).await {
                                continue;
                            }
                        }
                        let text = match serde_json::to_string(&event) {
                            Ok(text) => text,
                            Err(err) => {
                                tracing::warn!(error = %err, "failed to serialize platform event");
                                continue;
                            }
                        };
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        let payload = json!({
                            "skipped": skipped,
                            "message": "events were dropped; refetch current state",
                        });
                        let mut event = fleet_core_protocol::connection::PlatformEvent::new(
                            "platform.lagged",
                            payload,
                        );
                        event.machine_id = None;
                        let text = match serde_json::to_string(&event) {
                            Ok(text) => text,
                            Err(err) => {
                                tracing::warn!(error = %err, "failed to serialize lag notice");
                                continue;
                            }
                        };
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Closed) => break,
                }
            }
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            _ = reauth.tick() => {
                match resolve_principal(&state, &credentials).await {
                    Ok(current) if same_identity(&principal, &current) => {}
                    Ok(_) => break,
                    Err(_) => {
                        tracing::debug!("event socket credential is no longer valid");
                        break;
                    }
                }
            }
            message = stream.next() => {
                match message {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => {}
                    Some(Err(err)) => {
                        tracing::debug!(error = %err, "browser event socket error");
                        break;
                    }
                }
            }
        }
    }
}

async fn event_visible(state: &AppState, principal: &Principal, machine_id: &str) -> bool {
    if principal.is_legacy_admin() {
        return true;
    }
    let org_id = match state.event_index.machine_org(machine_id) {
        Some(org_id) => Some(org_id),
        None => {
            let org_id = state.db.machine_orgs.get(machine_id).await.ok().flatten();
            if let Some(org_id) = &org_id {
                state.event_index.set_machine_org(machine_id, org_id);
            }
            org_id
        }
    };
    principal.can_access_org(org_id.as_deref())
}

fn same_identity(left: &Principal, right: &Principal) -> bool {
    match (&left.kind, &right.kind) {
        (
            crate::middleware::auth::PrincipalKind::LegacyAdmin,
            crate::middleware::auth::PrincipalKind::LegacyAdmin,
        ) => true,
        (
            crate::middleware::auth::PrincipalKind::User(left),
            crate::middleware::auth::PrincipalKind::User(right),
        ) => left.id == right.id && left.org_id == right.org_id && left.role == right.role,
        _ => false,
    }
}
