//! ACP connections: spawn, initialize, prompt, cancel and close.
//!
//! Handlers are registered before the connection is established so updates
//! emitted during the handshake are never lost. A single connection always
//! uses exactly one negotiated protocol version; version fallback requires a
//! fresh connection and a fresh process.

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::v1::schema as v1s;
use agent_client_protocol::v1::ConnectionTo as V1ConnectionTo;
use agent_client_protocol::v2::schema as v2s;
use agent_client_protocol::v2::{ConnectionTo as V2ConnectionTo, InitializeOutcome};
use agent_client_protocol::{
    on_receive_notification, on_receive_request, schema::ProtocolVersion, AcpAgent, Agent, Client,
};
use fleet_core_protocol::agent::{
    AgentCapabilities, PermissionDecision, PermissionStatus, QuestionDecision, QuestionStatus,
    SessionStatus,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::driver::acp::capabilities::{map_v1, map_v2};
use crate::driver::acp::client::{
    handle_read_text_file, handle_write_text_file, v1_client_capabilities,
};
use crate::driver::acp::config::{AcpDriverConfig, AcpProtocolVersion};
use crate::driver::acp::mapper::{common, v1 as v1_mapper, v2 as v2_mapper};
use crate::driver::acp::permissions as v2_permissions;
use crate::driver::acp::process::AcpProcessConfig;
use crate::driver::acp::session::AcpSessionHandle;
use crate::driver::{AgentPrompt, PromptContent};
use crate::error::{Result, RuntimeError};

/// A live connection with its negotiated version.
pub enum AcpConnection {
    /// ACP v1 connection.
    V1(V1ConnectionTo<Agent>),
    /// ACP v2 connection.
    V2(V2ConnectionTo<Agent>),
}

impl AcpConnection {
    /// Negotiated protocol version.
    pub fn protocol(&self) -> AcpProtocolVersion {
        match self {
            Self::V1(_) => AcpProtocolVersion::V1,
            Self::V2(_) => AcpProtocolVersion::V2,
        }
    }
}

impl Clone for AcpConnection {
    fn clone(&self) -> Self {
        match self {
            Self::V1(connection) => Self::V1(connection.clone()),
            Self::V2(connection) => Self::V2(connection.clone()),
        }
    }
}

/// A connection that finished `initialize`.
pub struct SpawnedConnection {
    /// Protocol version.
    pub protocol: AcpProtocolVersion,
    /// Live connection.
    pub connection: Arc<AcpConnection>,
    /// Agent implementation info.
    pub agent_info: Value,
    /// Agent capabilities.
    pub capabilities: AgentCapabilities,
}

/// Outcome of a v2 spawn attempt.
pub enum V2SpawnOutcome {
    /// The agent negotiated v2.
    Ready(SpawnedConnection),
    /// The agent selected v1; the caller must reconnect with v1.
    LowerVersion,
    /// The agent rejected the requested version with `-32022`; the caller may
    /// reconnect with v1 when its policy allows it.
    UnsupportedVersion(String),
}

/// Spawn a v1 connection and run `initialize`.
pub async fn spawn_v1(
    process: AcpProcessConfig,
    session: Arc<AcpSessionHandle>,
    config: &AcpDriverConfig,
) -> Result<SpawnedConnection> {
    let (cx_tx, cx_rx) = oneshot::channel();
    let state_permission = Arc::clone(&session);
    let state_question = Arc::clone(&session);
    let state_read = Arc::clone(&session);
    let state_write = Arc::clone(&session);
    let state_terminal_create = Arc::clone(&session);
    let state_terminal_output = Arc::clone(&session);
    let state_terminal_wait = Arc::clone(&session);
    let state_terminal_kill = Arc::clone(&session);
    let state_terminal_release = Arc::clone(&session);
    let state_updates = Arc::clone(&session);
    let state_final = Arc::clone(&session);

    let task = tokio::spawn(async move {
        let result = Client
            .v1()
            .on_receive_request(
                async move |request: v1s::ReadTextFileRequest, responder, _cx| {
                    handle_read_text_file(Arc::clone(&state_read), request, responder).await;
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v1s::WriteTextFileRequest, responder, _cx| {
                    handle_write_text_file(Arc::clone(&state_write), request, responder).await;
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v1s::CreateTerminalRequest, responder, _cx| {
                    handle_terminal_create(Arc::clone(&state_terminal_create), request, responder)
                        .await;
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v1s::TerminalOutputRequest, responder, _cx| {
                    handle_terminal_output(Arc::clone(&state_terminal_output), request, responder);
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v1s::WaitForTerminalExitRequest, responder, _cx| {
                    // The SDK awaits request handlers inline on the
                    // connection actor; a long-running terminal must not
                    // block output, kill or session updates.
                    let session = Arc::clone(&state_terminal_wait);
                    tokio::spawn(async move {
                        handle_terminal_wait(session, request, responder).await;
                    });
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v1s::KillTerminalRequest, responder, _cx| {
                    handle_terminal_kill(Arc::clone(&state_terminal_kill), request, responder);
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v1s::ReleaseTerminalRequest, responder, _cx| {
                    handle_terminal_release(
                        Arc::clone(&state_terminal_release),
                        request,
                        responder,
                    );
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v1s::RequestPermissionRequest, responder, _cx| {
                    handle_v1_permission(Arc::clone(&state_permission), request, responder).await;
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v1s::CreateElicitationRequest, responder, _cx| {
                    handle_v1_elicitation(Arc::clone(&state_question), request, responder).await;
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_notification(
                async move |notification: v1s::SessionNotification, _cx| {
                    v1_mapper::apply_session_update(
                        &state_updates.emitter,
                        &state_updates.v1_state,
                        &notification,
                    );
                    Ok(())
                },
                on_receive_notification!(),
            )
            .connect_with(
                AcpAgent::new(process.agent),
                async move |cx: V1ConnectionTo<Agent>| {
                    let _ = cx_tx.send(cx.clone());
                    cx.incoming_closed().await;
                    Ok(())
                },
            )
            .await;
        on_connection_end(&state_final, result);
    });

    let connection = match tokio::time::timeout(config.startup_timeout, cx_rx).await {
        Ok(Ok(connection)) => connection,
        _ => {
            abort(task).await;
            return Err(RuntimeError::ProviderUnavailable(format!(
                "failed to start ACP agent `{}`",
                process.executable
            )));
        }
    };

    let initialize = v1s::InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(v1_client_capabilities())
        .client_info(v1s::Implementation::new(
            config.client_name.clone(),
            config.client_version.clone(),
        ));
    let response = match tokio::time::timeout(
        config.startup_timeout,
        connection.send_request(initialize).block_task(),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            abort(task).await;
            return Err(RuntimeError::ProviderUnavailable(format!(
                "ACP initialize failed: {error}"
            )));
        }
        Err(_) => {
            abort(task).await;
            return Err(RuntimeError::Timeout(
                "ACP initialize timed out".to_string(),
            ));
        }
    };
    if response.protocol_version != ProtocolVersion::V1 {
        abort(task).await;
        return Err(RuntimeError::Unsupported(format!(
            "ACP agent negotiated unsupported protocol version {}",
            response.protocol_version
        )));
    }

    let agent_info = json!({
        "name": "acp-agent",
        "protocol_version": response.protocol_version,
    });
    let capabilities = map_v1(&response.agent_capabilities);
    *session.protocol.lock() = Some(AcpProtocolVersion::V1);
    *session.agent_info.lock() = agent_info.clone();
    *session.capabilities.lock() = capabilities.clone();

    let connection = Arc::new(AcpConnection::V1(connection));
    *session.connection.lock() = Some(Arc::clone(&connection));
    *session.task.lock() = Some(task);

    Ok(SpawnedConnection {
        protocol: AcpProtocolVersion::V1,
        connection,
        agent_info,
        capabilities,
    })
}

/// Spawn a v2 connection and run `initialize`.
///
/// When the agent selects a lower protocol version, the connection is
/// abandoned and [`V2SpawnOutcome::LowerVersion`] is returned. The caller must
/// reconnect with v1 on a fresh process; a connection can never switch
/// versions after initialization.
pub async fn spawn_v2(
    process: AcpProcessConfig,
    session: Arc<AcpSessionHandle>,
    config: &AcpDriverConfig,
) -> Result<V2SpawnOutcome> {
    let (cx_tx, cx_rx) = oneshot::channel();
    let state_permission = Arc::clone(&session);
    let state_question = Arc::clone(&session);
    let state_updates = Arc::clone(&session);
    let state_final = Arc::clone(&session);

    let task = tokio::spawn(async move {
        let result = Client
            .v2()
            .on_receive_request(
                async move |request: v2s::RequestPermissionRequest, responder, _cx| {
                    handle_v2_permission(Arc::clone(&state_permission), request, responder).await;
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |request: v2s::CreateElicitationRequest, responder, _cx| {
                    handle_v2_elicitation(Arc::clone(&state_question), request, responder).await;
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_notification(
                async move |notification: v2s::UpdateSessionNotification, _cx| {
                    v2_mapper::apply_update(
                        &state_updates.emitter,
                        &state_updates.v2_state,
                        &notification,
                    );
                    Ok(())
                },
                on_receive_notification!(),
            )
            .connect_with(
                AcpAgent::new(process.agent),
                async move |cx: V2ConnectionTo<Agent>| {
                    let _ = cx_tx.send(cx.clone());
                    cx.incoming_closed().await;
                    Ok(())
                },
            )
            .await;
        on_connection_end(&state_final, result);
    });

    let connection = match tokio::time::timeout(config.startup_timeout, cx_rx).await {
        Ok(Ok(connection)) => connection,
        _ => {
            abort(task).await;
            return Err(RuntimeError::ProviderUnavailable(format!(
                "failed to start ACP agent `{}`",
                process.executable
            )));
        }
    };

    let initialize = v2s::InitializeRequest::new(
        ProtocolVersion::V2,
        v2s::Implementation::new(config.client_name.clone(), config.client_version.clone()),
    )
    .capabilities(v2s::ClientCapabilities::new().elicitation(
        v2s::ElicitationCapabilities::new().form(v2s::ElicitationFormCapabilities::new()),
    ));
    let outcome = match tokio::time::timeout(
        config.startup_timeout,
        connection.send_initialize(initialize).block_task(),
    )
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            abort(task).await;
            // -32022 is the protocol's "no supported version at or below the
            // requested one" error. A fallback is only safe for that error;
            // other initialization failures must not trigger a restart.
            if error.code.value() == -32022 {
                return Ok(V2SpawnOutcome::UnsupportedVersion(error.to_string()));
            }
            return Err(RuntimeError::ProviderUnavailable(format!(
                "ACP v2 initialize failed: {error}"
            )));
        }
        Err(_) => {
            abort(task).await;
            return Err(RuntimeError::Timeout(
                "ACP v2 initialize timed out".to_string(),
            ));
        }
    };

    let response = match outcome {
        InitializeOutcome::Initialized(response) => response,
        InitializeOutcome::LowerVersion(_) => {
            abort(task).await;
            return Ok(V2SpawnOutcome::LowerVersion);
        }
        other => {
            abort(task).await;
            return Err(RuntimeError::Unsupported(format!(
                "ACP v2 initialize returned an unsupported outcome: {other:?}"
            )));
        }
    };
    if response.protocol_version != ProtocolVersion::V2 {
        abort(task).await;
        return Err(RuntimeError::Unsupported(format!(
            "ACP agent negotiated unsupported protocol version {}",
            response.protocol_version
        )));
    }

    let agent_info = serde_json::to_value(&response.info).unwrap_or(Value::Null);
    let capabilities = map_v2(&response.capabilities);
    *session.protocol.lock() = Some(AcpProtocolVersion::V2);
    *session.agent_info.lock() = agent_info.clone();
    *session.capabilities.lock() = capabilities.clone();

    let connection = Arc::new(AcpConnection::V2(connection));
    *session.connection.lock() = Some(Arc::clone(&connection));
    *session.task.lock() = Some(task);

    Ok(V2SpawnOutcome::Ready(SpawnedConnection {
        protocol: AcpProtocolVersion::V2,
        connection,
        agent_info,
        capabilities,
    }))
}

/// Decode v1 MCP server descriptors from session `extra`.
fn mcp_servers_v1(values: &[Value]) -> Result<Vec<v1s::McpServer>> {
    values
        .iter()
        .cloned()
        .map(|value| {
            serde_json::from_value(value).map_err(|error| {
                RuntimeError::InvalidRequest(format!("invalid MCP server configuration: {error}"))
            })
        })
        .collect()
}

/// Decode v2 MCP server descriptors from session `extra`.
fn mcp_servers_v2(values: &[Value]) -> Result<Vec<v2s::McpServer>> {
    values
        .iter()
        .cloned()
        .map(|value| {
            serde_json::from_value(value).map_err(|error| {
                RuntimeError::InvalidRequest(format!("invalid MCP server configuration: {error}"))
            })
        })
        .collect()
}

/// Create a native session for an ACP connection.
pub async fn new_native_session(
    connection: &AcpConnection,
    cwd: &str,
    timeout: Duration,
    mcp_servers: &[Value],
) -> Result<String> {
    match connection {
        AcpConnection::V1(connection) => {
            let request = v1s::NewSessionRequest::new(cwd.to_string())
                .mcp_servers(mcp_servers_v1(mcp_servers)?);
            match tokio::time::timeout(timeout, connection.send_request(request).block_task()).await
            {
                Ok(Ok(response)) => Ok(response.session_id.to_string()),
                Ok(Err(error)) => Err(RuntimeError::ProviderUnavailable(format!(
                    "ACP session/new failed: {error}"
                ))),
                Err(_) => Err(RuntimeError::Timeout(
                    "ACP session/new timed out".to_string(),
                )),
            }
        }
        AcpConnection::V2(connection) => {
            let request = v2s::NewSessionRequest::new(cwd.to_string())
                .mcp_servers(mcp_servers_v2(mcp_servers)?);
            match tokio::time::timeout(timeout, connection.send_request(request).block_task()).await
            {
                Ok(Ok(response)) => Ok(response.session_id.to_string()),
                Ok(Err(error)) => Err(RuntimeError::ProviderUnavailable(format!(
                    "ACP v2 session/new failed: {error}"
                ))),
                Err(_) => Err(RuntimeError::Timeout(
                    "ACP v2 session/new timed out".to_string(),
                )),
            }
        }
    }
}

/// Send a prompt.
///
/// v1 returns immediately after dispatching the request; the response drives
/// the turn in the background, matching the existing v1 adapter. v2 waits for
/// the prompt response because it carries the accepted message id, but the
/// response still only means "inserted", never "turn completed".
pub async fn send_prompt(
    connection: &AcpConnection,
    session: &Arc<AcpSessionHandle>,
    native_session_id: &str,
    prompt: &AgentPrompt,
    accept_timeout: Duration,
) -> Result<Option<String>> {
    match connection {
        AcpConnection::V1(connection) => {
            let request =
                v1s::PromptRequest::new(native_session_id.to_string(), prompt_blocks_v1(prompt));
            let connection = connection.clone();
            let session = Arc::clone(session);
            tokio::spawn(async move {
                match connection.send_request(request).block_task().await {
                    Ok(response) => {
                        v1_mapper::complete_open_messages(&session.emitter, &session.v1_state);
                        if matches!(response.stop_reason, v1s::StopReason::Cancelled) {
                            session.emitter.emit(
                                fleet_core_protocol::agent::AgentEventType::TurnInterrupted,
                                json!({ "status": "interrupted" }),
                            );
                            v1_mapper::emit_status(&session.emitter, SessionStatus::Interrupted);
                        } else {
                            session.emitter.emit(
                                fleet_core_protocol::agent::AgentEventType::TurnCompleted,
                                json!({ "status": "completed" }),
                            );
                            v1_mapper::emit_status(
                                &session.emitter,
                                SessionStatus::WaitingForInput,
                            );
                        }
                    }
                    Err(error) => {
                        session.emitter.emit(
                            fleet_core_protocol::agent::AgentEventType::TurnFailed,
                            json!({ "error": error.to_string() }),
                        );
                        v1_mapper::emit_status(&session.emitter, SessionStatus::Failed);
                    }
                }
            });
            Ok(None)
        }
        AcpConnection::V2(connection) => {
            let request =
                v2s::PromptRequest::new(native_session_id.to_string(), prompt_blocks_v2(prompt));
            match tokio::time::timeout(
                accept_timeout,
                connection.send_request(request).block_task(),
            )
            .await
            {
                Ok(Ok(response)) => Ok(Some(response.message_id.to_string())),
                Ok(Err(error)) => Err(RuntimeError::provider(
                    "prompt_rejected",
                    format!("ACP v2 session/prompt failed: {error}"),
                )),
                Err(_) => {
                    // The prompt may have been inserted even though the
                    // response was late. Do not resend; the state updates are
                    // authoritative for the turn.
                    tracing::warn!(
                        session_id = %session.fleet_session_id,
                        "ACP v2 prompt response timed out; continuing to consume updates"
                    );
                    Ok(None)
                }
            }
        }
    }
}

/// Resume a native session on a freshly spawned connection.
pub async fn resume_session(
    connection: &AcpConnection,
    native_session_id: &str,
    cwd: &str,
    timeout: Duration,
    mcp_servers: &[Value],
) -> Result<()> {
    match connection {
        AcpConnection::V1(connection) => {
            let request = v1s::ResumeSessionRequest::new(native_session_id.to_string(), cwd)
                .mcp_servers(mcp_servers_v1(mcp_servers)?);
            match tokio::time::timeout(timeout, connection.send_request(request).block_task()).await
            {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(error)) => Err(RuntimeError::ProviderUnavailable(format!(
                    "ACP session/resume failed: {error}"
                ))),
                Err(_) => Err(RuntimeError::Timeout(
                    "ACP session/resume timed out".to_string(),
                )),
            }
        }
        AcpConnection::V2(connection) => {
            let request = v2s::ResumeSessionRequest::new(
                v2s::SessionId::new(native_session_id.to_string()),
                v2s::AbsolutePath::new(cwd.to_string()),
            )
            .mcp_servers(mcp_servers_v2(mcp_servers)?);
            match tokio::time::timeout(timeout, connection.send_request(request).block_task()).await
            {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(error)) => Err(RuntimeError::ProviderUnavailable(format!(
                    "ACP v2 session/resume failed: {error}"
                ))),
                Err(_) => Err(RuntimeError::Timeout(
                    "ACP v2 session/resume timed out".to_string(),
                )),
            }
        }
    }
}

/// Cancel the current foreground work.
pub async fn cancel(connection: &AcpConnection, native_session_id: &str) -> Result<()> {
    match connection {
        AcpConnection::V1(connection) => connection
            .send_notification(v1s::CancelNotification::new(native_session_id.to_string()))
            .map_err(|error| {
                RuntimeError::ProviderUnavailable(format!("failed to cancel ACP turn: {error}"))
            }),
        AcpConnection::V2(connection) => connection
            .send_notification(v2s::CancelSessionNotification::new(
                native_session_id.to_string(),
            ))
            .map_err(|error| {
                RuntimeError::ProviderUnavailable(format!("failed to cancel ACP v2 work: {error}"))
            }),
    }
}

/// Close the native session, when the protocol supports it.
pub async fn close(
    connection: &AcpConnection,
    native_session_id: &str,
    timeout: Duration,
) -> Result<()> {
    match connection {
        AcpConnection::V1(connection) => {
            let request = v1s::CloseSessionRequest::new(native_session_id.to_string());
            match tokio::time::timeout(timeout, connection.send_request(request).block_task()).await
            {
                Ok(Ok(_)) | Ok(Err(_)) | Err(_) => Ok(()),
            }
        }
        AcpConnection::V2(connection) => {
            let request = v2s::CloseSessionRequest::new(native_session_id.to_string());
            match tokio::time::timeout(timeout, connection.send_request(request).block_task()).await
            {
                Ok(Ok(_)) | Ok(Err(_)) | Err(_) => Ok(()),
            }
        }
    }
}

/// Abort a connection task and wait for it to finish.
pub async fn abort(task: JoinHandle<()>) {
    task.abort();
    let _ = task.await;
}

fn on_connection_end(
    session: &AcpSessionHandle,
    result: std::result::Result<(), agent_client_protocol::Error>,
) {
    session.terminals.clear();
    session
        .emitter
        .resolve_all_pending_questions(QuestionStatus::Cancelled);
    session
        .emitter
        .resolve_all_pending_permissions(PermissionStatus::Cancelled);
    if session.is_closing() {
        return;
    }
    if let Err(error) = &result {
        tracing::warn!(error = %error, "ACP connection ended with an error");
    }
    session.emitter.emit(
        fleet_core_protocol::agent::AgentEventType::SessionError,
        json!({"error": {
            "code": "acp_connection_lost",
            "message": result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "ACP agent connection closed".to_string())
        }}),
    );
    common::emit_status(&session.emitter, SessionStatus::Failed);
}

async fn handle_terminal_create(
    session: Arc<AcpSessionHandle>,
    request: v1s::CreateTerminalRequest,
    responder: agent_client_protocol::v1::Responder<v1s::CreateTerminalResponse>,
) {
    if !super::terminal::request_matches_session(&session, &request.session_id.to_string()) {
        let _ = responder.respond_with_error(
            agent_client_protocol::Error::invalid_params().data("unknown terminal session"),
        );
        return;
    }
    match session.terminals.create(&session, &request).await {
        Ok(response) => {
            let _ = responder.respond(response);
        }
        Err(error) => {
            let _ = responder
                .respond_with_error(agent_client_protocol::Error::invalid_params().data(error));
        }
    }
}

fn handle_terminal_output(
    session: Arc<AcpSessionHandle>,
    request: v1s::TerminalOutputRequest,
    responder: agent_client_protocol::v1::Responder<v1s::TerminalOutputResponse>,
) {
    if !super::terminal::request_matches_session(&session, &request.session_id.to_string()) {
        let _ = responder.respond_with_error(
            agent_client_protocol::Error::invalid_params().data("unknown terminal session"),
        );
        return;
    }
    match session.terminals.output(&request.terminal_id.to_string()) {
        Ok(response) => {
            let _ = responder.respond(response);
        }
        Err(error) => {
            let _ = responder
                .respond_with_error(agent_client_protocol::Error::invalid_params().data(error));
        }
    }
}

async fn handle_terminal_wait(
    session: Arc<AcpSessionHandle>,
    request: v1s::WaitForTerminalExitRequest,
    responder: agent_client_protocol::v1::Responder<v1s::WaitForTerminalExitResponse>,
) {
    if !super::terminal::request_matches_session(&session, &request.session_id.to_string()) {
        let _ = responder.respond_with_error(
            agent_client_protocol::Error::invalid_params().data("unknown terminal session"),
        );
        return;
    }
    match session
        .terminals
        .wait_for_exit(&request.terminal_id.to_string())
        .await
    {
        Ok(status) => {
            let _ = responder.respond(v1s::WaitForTerminalExitResponse::new(status));
        }
        Err(error) => {
            let _ = responder
                .respond_with_error(agent_client_protocol::Error::invalid_params().data(error));
        }
    }
}

fn handle_terminal_kill(
    session: Arc<AcpSessionHandle>,
    request: v1s::KillTerminalRequest,
    responder: agent_client_protocol::v1::Responder<v1s::KillTerminalResponse>,
) {
    if !super::terminal::request_matches_session(&session, &request.session_id.to_string()) {
        let _ = responder.respond_with_error(
            agent_client_protocol::Error::invalid_params().data("unknown terminal session"),
        );
        return;
    }
    match session.terminals.kill(&request.terminal_id.to_string()) {
        Ok(()) => {
            let _ = responder.respond(v1s::KillTerminalResponse::new());
        }
        Err(error) => {
            let _ = responder
                .respond_with_error(agent_client_protocol::Error::invalid_params().data(error));
        }
    }
}

fn handle_terminal_release(
    session: Arc<AcpSessionHandle>,
    request: v1s::ReleaseTerminalRequest,
    responder: agent_client_protocol::v1::Responder<v1s::ReleaseTerminalResponse>,
) {
    if !super::terminal::request_matches_session(&session, &request.session_id.to_string()) {
        let _ = responder.respond_with_error(
            agent_client_protocol::Error::invalid_params().data("unknown terminal session"),
        );
        return;
    }
    match session.terminals.release(&request.terminal_id.to_string()) {
        Ok(()) => {
            let _ = responder.respond(v1s::ReleaseTerminalResponse::new());
        }
        Err(error) => {
            let _ = responder
                .respond_with_error(agent_client_protocol::Error::invalid_params().data(error));
        }
    }
}

async fn handle_v1_permission(
    session: Arc<AcpSessionHandle>,
    request: v1s::RequestPermissionRequest,
    responder: agent_client_protocol::v1::Responder<v1s::RequestPermissionResponse>,
) {
    let mut permission = v1_mapper::map_permission_request(&request);
    permission.session_id = session.fleet_session_id.clone();
    let pending = permission.clone();
    let emitter = session.emitter.clone();
    let receiver = emitter.request_permission(permission);
    let timeout = session.permission_timeout;
    tokio::spawn(async move {
        let decision = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(decision)) => decision,
            _ => {
                emitter.resolve_permission_locally(&pending.id, PermissionStatus::TimedOut);
                PermissionDecision {
                    permission_id: pending.id.clone(),
                    session_id: None,
                    approve: false,
                    option_id: None,
                    message: None,
                }
            }
        };
        let _ = responder.respond(v1_mapper::map_permission_response(&pending, &decision));
    });
}

async fn handle_v1_elicitation(
    session: Arc<AcpSessionHandle>,
    request: v1s::CreateElicitationRequest,
    responder: agent_client_protocol::v1::Responder<v1s::CreateElicitationResponse>,
) {
    let mut question = match v1_mapper::map_elicitation_request(&request) {
        Ok(question) => question,
        Err(error) => {
            tracing::warn!(error = %error, "declining unsupported ACP elicitation request");
            let _ = responder.respond(v1s::CreateElicitationResponse::new(
                v1s::ElicitationAction::Decline,
            ));
            return;
        }
    };
    question.session_id = session.fleet_session_id.clone();
    let pending = question.clone();
    let emitter = session.emitter.clone();
    let receiver = emitter.request_question(question);
    let timeout = session.question_timeout;
    tokio::spawn(async move {
        let decision = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(decision)) => decision,
            _ => {
                emitter.resolve_question_locally(&pending.id, QuestionStatus::TimedOut);
                QuestionDecision {
                    question_id: pending.id.clone(),
                    session_id: None,
                    answers: Vec::new(),
                    reject: true,
                    message: None,
                }
            }
        };
        if responder
            .respond(v1_mapper::map_elicitation_response(&pending, &decision))
            .is_err()
        {
            emitter.fail_question_delivery(&pending.id);
        }
    });
}

async fn handle_v2_permission(
    session: Arc<AcpSessionHandle>,
    request: v2s::RequestPermissionRequest,
    responder: agent_client_protocol::v2::Responder<v2s::RequestPermissionResponse>,
) {
    let mut permission = v2_permissions::map_permission_request(&request);
    permission.session_id = session.fleet_session_id.clone();
    let pending = permission.clone();
    let emitter = session.emitter.clone();
    let receiver = emitter.request_permission(permission);
    let timeout = session.permission_timeout;
    tokio::spawn(async move {
        let decision = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(decision)) => decision,
            _ => {
                emitter.resolve_permission_locally(&pending.id, PermissionStatus::TimedOut);
                PermissionDecision {
                    permission_id: pending.id.clone(),
                    session_id: None,
                    approve: false,
                    option_id: None,
                    message: None,
                }
            }
        };
        let _ = responder.respond(v2_permissions::map_permission_response(&pending, &decision));
    });
}

async fn handle_v2_elicitation(
    session: Arc<AcpSessionHandle>,
    request: v2s::CreateElicitationRequest,
    responder: agent_client_protocol::v2::Responder<v2s::CreateElicitationResponse>,
) {
    let mut question = match v2_permissions::map_elicitation_request(&request) {
        Ok(question) => question,
        Err(error) => {
            tracing::warn!(error = %error, "declining unsupported ACP v2 elicitation request");
            let _ = responder.respond(v2s::CreateElicitationResponse::new(
                v2s::ElicitationAction::Decline,
            ));
            return;
        }
    };
    question.session_id = session.fleet_session_id.clone();
    let pending = question.clone();
    let emitter = session.emitter.clone();
    let receiver = emitter.request_question(question);
    let timeout = session.question_timeout;
    tokio::spawn(async move {
        let decision = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(decision)) => decision,
            _ => {
                emitter.resolve_question_locally(&pending.id, QuestionStatus::TimedOut);
                QuestionDecision {
                    question_id: pending.id.clone(),
                    session_id: None,
                    answers: Vec::new(),
                    reject: true,
                    message: None,
                }
            }
        };
        if responder
            .respond(v2_permissions::map_elicitation_response(
                &pending, &decision,
            ))
            .is_err()
        {
            emitter.fail_question_delivery(&pending.id);
        }
    });
}

fn prompt_blocks_v1(prompt: &AgentPrompt) -> Vec<v1s::ContentBlock> {
    prompt
        .content
        .iter()
        .map(|part| match part {
            PromptContent::Text(text) => {
                v1s::ContentBlock::Text(v1s::TextContent::new(text.clone()))
            }
            PromptContent::Image(image) => v1s::ContentBlock::Image(v1s::ImageContent::new(
                image.data.clone(),
                image.mime_type.clone(),
            )),
        })
        .collect()
}

fn prompt_blocks_v2(prompt: &AgentPrompt) -> Vec<v2s::ContentBlock> {
    prompt
        .content
        .iter()
        .map(|part| match part {
            PromptContent::Text(text) => {
                v2s::ContentBlock::Text(v2s::TextContent::new(text.clone()))
            }
            PromptContent::Image(image) => v2s::ContentBlock::Image(v2s::ImageContent::new(
                image.data.clone(),
                image.mime_type.clone(),
            )),
        })
        .collect()
}
