//! Browser event WebSocket at `/api/v1/events`.
//!
//! Browsers cannot attach an `Authorization` header to a WebSocket upgrade, so
//! this endpoint accepts either that header or a `?token=` query parameter.

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
    let header_ok = state
        .auth
        .verify_bearer(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
        )
        .is_ok();
    let query_ok = query
        .token
        .as_deref()
        .map(|token| state.auth.verify_raw(token).is_ok())
        .unwrap_or(false);

    if !header_ok && !query_ok {
        return Err(AppError::unauthorized(
            "missing or invalid bearer token or token query parameter",
        ));
    }

    Ok(ws.on_upgrade(move |socket| stream_events(socket, state)))
}

async fn stream_events(socket: WebSocket, state: AppState) {
    // Single-user MVP: no event filtering. The filter is kept explicit so
    // per-user authorization can be added without changing the transport.
    let filter = EventFilter::new();
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
