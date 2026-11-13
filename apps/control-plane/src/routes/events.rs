//! Browser event WebSocket at `/api/v1/events`.
//!
//! Browsers cannot attach an `Authorization` header to a WebSocket upgrade, so
//! this endpoint accepts either that header or a `?token=` query parameter.

use std::collections::HashSet;

use agent_fleet_common::error_codes;
use agent_fleet_event_bus::EventFilter;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
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

    Ok(ws.on_upgrade(move |socket| stream_events(socket, state, principal)))
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

async fn stream_events(socket: WebSocket, state: AppState, principal: Principal) {
    let filter = EventFilter::new();
    // Snapshot the caller's machine scope for this connection. A machine moved
    // to another organization stops emitting events here after a reconnect.
    let visible_machines: Option<HashSet<String>> = if principal.is_legacy_admin() {
        None
    } else {
        match state
            .db
            .machine_ids_for_org(principal.org_id().unwrap_or_default())
            .await
        {
            Ok(ids) => Some(ids.into_iter().collect()),
            Err(error) => {
                tracing::error!(error = %error, "failed to load machine scope for events");
                let payload = json!({
                    "type": "platform.error",
                    "payload": {
                        "code": error_codes::INTERNAL,
                        "message": "failed to load event scope",
                    },
                });
                let mut socket = socket;
                let _ = socket.send(Message::Text(payload.to_string().into())).await;
                return;
            }
        }
    };
    let mut events = state.events.subscribe();
    let (mut sink, mut stream) = socket.split();

    loop {
        tokio::select! {
            received = events.recv() => {
                match received {
                    Ok(event) => {
                        if !filter.matches(&event) {
                            continue;
                        }
                        if let Some(visible) = &visible_machines {
                            if let Some(machine_id) = event.machine_id.as_deref() {
                                if !visible.contains(machine_id) {
                                    continue;
                                }
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
                            "type": "platform.lagged",
                            "payload": { "skipped": skipped },
                        });
                        let text = payload.to_string();
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Closed) => break,
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
