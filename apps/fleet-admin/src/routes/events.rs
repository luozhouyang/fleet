//! Browser event WebSocket at `/api/v1/events` and its ticket endpoint.
//!
//! The socket is authorized by a single-use ticket minted by
//! `POST /api/v1/realtime/ticket`. A ticket is bound to one user and one
//! organization and expires after 60 seconds, so a leaked URL cannot be
//! replayed. A bearer token is still accepted for the CLI and tests.
//!
//! The connection re-validates membership every 15 seconds and stops as soon as
//! the user is removed, disabled or the organization disappears. It also emits a
//! `platform.heartbeat` event on the same interval so browser clients can detect
//! half-open sockets that never deliver a `close` event.

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use axum::Json;
use chrono::Duration as ChronoDuration;
use fleet_admin_eventbus::EventFilter;
use fleet_core_auth::hash_token;
use fleet_core_common::now;
use fleet_core_protocol::connection::PlatformEvent;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::error::AppError;
use crate::middleware::auth::{build_principal, resolve_token, Principal};
use crate::state::AppState;

/// How long a freshly minted ticket stays valid.
pub const TICKET_TTL_SECS: i64 = 60;

/// Interval between `platform.heartbeat` events. Browser watchdogs close and
/// reconnect a socket that stays silent for several intervals.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 15;

/// Query parameters accepted by the ticket endpoint.
#[derive(Debug, Deserialize, Default)]
pub struct TicketQuery {
    /// Organization to receive events for. Defaults to the active organization.
    #[serde(default)]
    pub org_id: Option<String>,
}

/// Ticket handed to the browser for a WebSocket upgrade.
#[derive(Debug, serde::Serialize)]
pub struct RealtimeTicket {
    /// Opaque single-use ticket.
    pub ticket: String,
    /// Organization the ticket is bound to.
    pub org_id: String,
    /// Seconds until the ticket expires.
    pub expires_in: i64,
}

/// Query parameters accepted by the event socket.
#[derive(Debug, Deserialize, Default)]
pub struct EventSocketQuery {
    /// Single-use ticket returned by `POST /api/v1/realtime/ticket`.
    #[serde(default)]
    pub ticket: Option<String>,
}

/// `POST /api/v1/realtime/ticket`
pub async fn create_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TicketQuery>,
) -> Result<Json<RealtimeTicket>, AppError> {
    crate::routes::session::ensure_origin(&state, &headers)?;
    // This route sits outside `require_auth` so the handler resolves the
    // principal itself; enforce the same CSRF rule as the protected router.
    if !crate::middleware::auth::has_bearer_credentials(&headers)
        && !crate::auth::csrf_valid(&headers)
    {
        return Err(AppError::forbidden("missing or invalid CSRF token"));
    }
    let principal = crate::middleware::auth::resolve_principal(&state, &headers).await?;
    let user = principal
        .user()
        .ok_or_else(|| AppError::forbidden("the bootstrap token cannot open the event socket"))?;
    let requested = query
        .org_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .or(principal.org_id())
        .ok_or_else(|| AppError::forbidden("no active organization"))?;
    if principal.role_in(requested).is_none() {
        return Err(AppError::forbidden(
            "you are not a member of that organization",
        ));
    }

    let ticket = crate::auth::random_token(32);
    // Bind the socket to the session only when the session authenticated this
    // request; a bearer request stays valid independently of any cookie.
    let session_hash = if crate::middleware::auth::has_bearer_credentials(&headers) {
        None
    } else {
        crate::auth::session_cookie_hash_from(&headers)
    };
    state
        .db
        .realtime_tickets
        .create(
            &hash_token(&ticket),
            &user.id,
            requested,
            session_hash.as_deref(),
            now() + ChronoDuration::seconds(TICKET_TTL_SECS),
        )
        .await?;
    Ok(Json(RealtimeTicket {
        ticket,
        org_id: requested.to_string(),
        expires_in: TICKET_TTL_SECS,
    }))
}

/// `GET /api/v1/events` (WebSocket upgrade).
pub async fn events_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventSocketQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    crate::routes::session::ensure_origin(&state, &headers)?;
    if let Some(credentials) = bearer_credentials(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    ) {
        let principal = resolve_token(
            &state,
            credentials,
            crate::middleware::auth::requested_org(&headers),
        )
        .await?;
        if principal.is_legacy_admin() {
            return Err(AppError::forbidden(
                "the bootstrap token cannot open the event socket",
            ));
        }
        let credentials = credentials.to_string();
        return Ok(ws.on_upgrade(move |socket| {
            stream_events(socket, state, principal, Some(credentials), None)
        }));
    }

    let ticket = query
        .ticket
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::unauthorized("missing event socket ticket"))?;
    let Some((user_id, org_id, session_hash)) = state
        .db
        .realtime_tickets
        .consume(&hash_token(ticket))
        .await?
    else {
        return Err(AppError::unauthorized("ticket is invalid, used or expired"));
    };
    // A ticket minted by a browser session dies with that session, even if it
    // is presented within its 60-second lifetime.
    if let Some(hash) = &session_hash {
        if state.db.auth_sessions.active(hash).await?.is_none() {
            return Err(AppError::unauthorized("session is expired or revoked"));
        }
    }
    let user = state
        .db
        .users
        .get(&user_id)
        .await?
        .ok_or_else(|| AppError::unauthorized("ticket user no longer exists"))?;
    let principal = build_principal(&state, user, Some(&org_id)).await?;
    Ok(ws.on_upgrade(move |socket| stream_events(socket, state, principal, None, session_hash)))
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
    credentials: Option<String>,
    session_hash: Option<String>,
) {
    // The upgrade succeeded; hold the gauge open for the life of the stream.
    let _connection = state.metrics.websocket_connection_guard();
    let filter = EventFilter::new();
    let mut events = state.events.subscribe();
    // Re-authenticate periodically so deleted users and rotated tokens stop
    // receiving events even if they keep an old WebSocket open.
    let mut reauth = tokio::time::interval(Duration::from_secs(15));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    let (mut sink, mut stream) = socket.split();

    loop {
        tokio::select! {
            received = events.recv() => {
                match received {
                    Ok(event) => {
                        if !filter.matches(&event) {
                            continue;
                        }
                        if !event_visible(&state, &principal, &event).await {
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
                        state.events.record_dropped(skipped);
                        state.metrics.record_event_bus_dropped(skipped);
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
            _ = heartbeat.tick() => {
                let event = fleet_core_protocol::connection::PlatformEvent::new(
                    "platform.heartbeat",
                    json!({ "server_time": now() }),
                );
                let text = match serde_json::to_string(&event) {
                    Ok(text) => text,
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to serialize heartbeat event");
                        continue;
                    }
                };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            _ = reauth.tick() => {
                match refresh_principal(
                    &state,
                    &principal,
                    credentials.as_deref(),
                    session_hash.as_deref(),
                )
                .await
                {
                    Some(current) if same_identity(&principal, &current) => {}
                    _ => {
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

/// Re-resolve the principal 15 seconds after authentication.
///
/// Returns `None` instead of an error when the credential can no longer be
/// resolved; the caller then closes the socket.
async fn refresh_principal(
    state: &AppState,
    principal: &Principal,
    credentials: Option<&str>,
    session_hash: Option<&str>,
) -> Option<Principal> {
    let Some(user) = principal.user() else {
        // The bootstrap token never expires; keep the socket open.
        return Some(principal.clone());
    };
    let org_id = principal.org_id()?.to_string();

    if let Some(credentials) = credentials {
        return match resolve_token(state, credentials, Some(&org_id)).await {
            Ok(principal) => Some(principal),
            Err(error) => {
                tracing::debug!(error = ?error, "event socket credential is no longer valid");
                None
            }
        };
    }

    // A ticket minted by a browser session is only valid while that session is.
    if let Some(hash) = session_hash {
        let session = match state.db.auth_sessions.active(hash).await {
            Ok(Some(session)) => session,
            Ok(None) => return None,
            Err(error) => {
                tracing::debug!(error = %error, "event socket session lookup failed");
                return None;
            }
        };
        let idle_days =
            i64::try_from(state.config.auth.session_idle_ttl_days).unwrap_or(i64::MAX / 2);
        if session.is_idle_for(ChronoDuration::days(idle_days)) {
            let _ = state.db.auth_sessions.revoke(hash).await;
            return None;
        }
        if let Err(error) = state.db.auth_sessions.touch(hash).await {
            tracing::debug!(error = %error, "failed to record session activity");
        }
    }

    match state.db.users.get(&user.id).await {
        Ok(Some(user)) => match build_principal(state, user, Some(&org_id)).await {
            Ok(principal) => Some(principal),
            Err(error) => {
                tracing::debug!(error = ?error, "event socket credential is no longer valid");
                None
            }
        },
        Ok(None) => None,
        Err(error) => {
            tracing::debug!(error = %error, "event socket re-auth failed");
            None
        }
    }
}

/// Whether this event belongs to the socket's organization.
///
/// The ticket (or the active-organization header on a bearer connection)
/// scopes the socket to exactly one organization. Membership in other
/// organizations never widens that: switching organizations opens a new socket.
async fn event_visible(state: &AppState, principal: &Principal, event: &PlatformEvent) -> bool {
    if matches!(
        event.event_type.as_str(),
        "platform.heartbeat" | "platform.lagged"
    ) {
        return true;
    }
    if principal.is_legacy_admin() {
        return false;
    }
    let org_id = match event.org_id.as_deref() {
        Some(org_id) => Some(org_id.to_string()),
        None => match event.machine_id.as_deref() {
            Some(machine_id) => {
                crate::services::event_service::machine_org_for_event(state, machine_id).await
            }
            None => None,
        },
    };
    org_id
        .as_deref()
        .is_some_and(|owner| principal.org_id() == Some(owner))
}

fn same_identity(left: &Principal, right: &Principal) -> bool {
    if left.is_legacy_admin() != right.is_legacy_admin() {
        return false;
    }
    if left.is_legacy_admin() {
        return true;
    }
    match (left.user(), right.user()) {
        (Some(left_user), Some(right_user)) => {
            left_user.id == right_user.id
                && left.org_id() == right.org_id()
                && left.role == right.role
                && left_user.disabled_at == right_user.disabled_at
        }
        _ => false,
    }
}
