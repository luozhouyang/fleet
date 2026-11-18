//! Integration tests for the generic ACP driver.
//!
//! The fixture is the same deterministic Node.js ACP v1 agent used by the
//! legacy provider tests. It lets the tests exercise real process spawning,
//! the JSON-RPC handshake, session creation, streaming, permissions,
//! elicitations, cancellation and v2-to-v1 negotiation fallback.

use std::sync::Arc;
use std::time::Duration;

use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEvent, AgentEventType, AgentPermissionRequest, AgentQuestionAnswer,
    AgentQuestionRequest, AgentStartConfig, PermissionDecision, QuestionDecision,
    SendMessageRequest, SessionStatus,
};
use fleet_daemon_runtime::agent::ProcessLaunchConfig;
use fleet_daemon_runtime::{
    AcpDriver, AcpDriverConfig, AcpProtocolPolicy, AgentDefinition, AgentRegistry, AgentRuntime,
    DriverRegistry,
};
use tokio::sync::broadcast;
use tokio::time::Instant;

fn fixture_path() -> String {
    format!(
        "{}/tests/fixtures/fake-acp-agent.mjs",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn definition(args: Vec<String>) -> AgentDefinition {
    let mut launch = ProcessLaunchConfig::new("node");
    launch.args = args;
    let mut definition = AgentDefinition::process("test-acp", "Test ACP", "acp", launch);
    definition.declared_capabilities = AgentCapabilities::full();
    definition
}

fn runtime_for(
    args: Vec<String>,
    protocol_policy: AcpProtocolPolicy,
    acp_v2_enabled: bool,
) -> AgentRuntime {
    let mut agents = AgentRegistry::new();
    agents.register(definition(args)).expect("definition");
    let mut drivers = DriverRegistry::new();
    let config = AcpDriverConfig {
        protocol_policy,
        acp_v2_enabled,
        ..AcpDriverConfig::default()
    };
    drivers
        .register(Arc::new(AcpDriver::new(config)))
        .expect("driver");
    AgentRuntime::from_components(agents, drivers, 4096)
}

fn start_config(cwd: &std::path::Path) -> AgentStartConfig {
    AgentStartConfig {
        cwd: cwd.to_string_lossy().to_string(),
        title: Some("acp driver test".to_string()),
        ..AgentStartConfig::default()
    }
}

async fn wait_for_status(runtime: &AgentRuntime, session_id: &str, expected: SessionStatus) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if runtime.get_session(session_id).await.unwrap().status == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "session did not become {expected}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn next_matching(
    receiver: &mut broadcast::Receiver<AgentEvent>,
    predicate: impl Fn(&AgentEvent) -> bool,
) -> AgentEvent {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let event = tokio::time::timeout_at(deadline, receiver.recv())
            .await
            .expect("timed out waiting for an ACP event")
            .expect("event bus closed");
        if predicate(&event) {
            return event;
        }
    }
}

#[tokio::test]
async fn acp_driver_v1_streams_prompts_and_answers_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(vec![fixture_path()], AcpProtocolPolicy::V1Only, false);
    let mut events = runtime.subscribe();

    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start ACP session");
    assert_eq!(session.status, SessionStatus::Running);
    assert_eq!(session.metadata["protocol"]["version"], 1);
    assert_eq!(session.metadata["agent"]["driver_id"], "acp");

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "hello".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send prompt");

    let delta = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::MessageDelta
    })
    .await;
    assert_eq!(delta.payload["text"], "Hello from ACP");

    let permission_event = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::PermissionRequested
    })
    .await;
    let permission: AgentPermissionRequest =
        serde_json::from_value(permission_event.payload["permission"].clone()).unwrap();
    assert_eq!(permission.tool_name, "Run tests");
    assert_eq!(permission.options[0].option_id, "allow_once");

    runtime
        .respond_permission(PermissionDecision {
            permission_id: permission.id,
            session_id: Some(session.id.clone()),
            approve: true,
            option_id: Some("allow_once".to_string()),
            message: None,
        })
        .await
        .expect("respond to permission");

    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;

    runtime.shutdown_session(&session.id).await.unwrap();
    assert!(runtime.instances().is_empty());
}

#[tokio::test]
async fn isolated_environment_launcher_runs_a_real_acp_session() {
    let temp = tempfile::tempdir().unwrap();
    let mut launch = ProcessLaunchConfig::new("node");
    launch.args = vec![fixture_path()];
    // An explicit allowlist selects Fleet's env-clear spawner instead of the
    // SDK default.
    launch.inherit_env = Some(vec!["PATH".to_string()]);
    let mut definition = AgentDefinition::process("test-acp", "Test ACP", "acp", launch);
    definition.declared_capabilities = AgentCapabilities::full();

    let mut agents = AgentRegistry::new();
    agents.register(definition).expect("definition");
    let mut drivers = DriverRegistry::new();
    drivers
        .register(Arc::new(AcpDriver::new(AcpDriverConfig {
            protocol_policy: AcpProtocolPolicy::V1Only,
            acp_v2_enabled: false,
            ..AcpDriverConfig::default()
        })))
        .expect("driver");
    let runtime = AgentRuntime::from_components(agents, drivers, 4096);
    let mut events = runtime.subscribe();

    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start ACP session with an isolated environment");
    assert_eq!(session.status, SessionStatus::Running);

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "hello".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send prompt");
    let delta = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::MessageDelta
    })
    .await;
    assert_eq!(delta.payload["text"], "Hello from ACP");

    runtime.interrupt(&session.id).await.expect("interrupt");
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_auto_falls_back_to_v1_when_agent_rejects_v2() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(vec![fixture_path()], AcpProtocolPolicy::Auto, true);
    let mut events = runtime.subscribe();

    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start ACP session with negotiation fallback");
    assert_eq!(
        session.metadata["protocol"]["version"], 1,
        "the v1-only fixture must be negotiated down to v1"
    );

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "hello".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send prompt");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::PermissionRequested
    })
    .await;
    runtime.interrupt(&session.id).await.expect("interrupt");
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_questions_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(
        vec![fixture_path(), "--question".to_string()],
        AcpProtocolPolicy::V1Only,
        false,
    );
    let mut events = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start");

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "ask".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send");

    let question_event = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::QuestionRequested
    })
    .await;
    let question: AgentQuestionRequest =
        serde_json::from_value(question_event.payload["question"].clone()).unwrap();
    assert_eq!(question.questions[0].id, "question_0");
    runtime
        .respond_question(QuestionDecision {
            question_id: question.id,
            session_id: Some(session.id.clone()),
            answers: vec![AgentQuestionAnswer {
                id: "question_0".into(),
                selected: vec!["staging".into()],
                custom: None,
            }],
            reject: false,
            message: None,
        })
        .await
        .expect("answer");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_v1_filesystem_requests_stay_inside_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("input.txt"), "from-agent\n").unwrap();
    let root_arg = format!("--fs-root={}", temp.path().display());
    let runtime = runtime_for(
        vec![fixture_path(), root_arg],
        AcpProtocolPolicy::V1Only,
        false,
    );
    let mut events = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start");

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "use fs".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::MessageDelta
            && event.payload["text"] == "fs done advertised=true"
    })
    .await;
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;

    assert_eq!(
        std::fs::read_to_string(temp.path().join("output.txt")).unwrap(),
        "echo:from-agent\n"
    );
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_v1_fs_scope_follows_the_workspace_root() {
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("repo");
    std::fs::create_dir(&session_dir).unwrap();
    std::fs::write(temp.path().join("input.txt"), "root-file\n").unwrap();
    let root_arg = format!("--fs-root={}", temp.path().display());
    let runtime = runtime_for(
        vec![fixture_path(), root_arg],
        AcpProtocolPolicy::V1Only,
        false,
    );
    let mut events = runtime.subscribe();
    let mut config = start_config(&session_dir);
    config.workspace_root = Some(temp.path().to_string_lossy().to_string());
    let session = runtime
        .start_session("machine_test", "test-acp", config)
        .await
        .expect("start");
    assert_eq!(session.cwd, session_dir.to_string_lossy());

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "use fs".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::MessageDelta
            && event.payload["text"] == "fs done advertised=true"
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(temp.path().join("output.txt")).unwrap(),
        "echo:root-file\n"
    );
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_v1_terminal_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(
        vec![fixture_path(), "--terminal".to_string()],
        AcpProtocolPolicy::V1Only,
        false,
    );
    let mut events = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start");
    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "run terminal".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::MessageDelta
            && event.payload["text"] == "terminal:hi advertised=true"
    })
    .await;
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_v1_rejects_filesystem_requests_outside_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(
        vec![fixture_path(), "--fs-escape".to_string()],
        AcpProtocolPolicy::V1Only,
        false,
    );
    let mut events = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start");
    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "escape".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::MessageDelta && event.payload["text"] == "fs denied"
    })
    .await;
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    runtime.shutdown_session(&session.id).await.unwrap();
}

fn fixture_v2_path() -> String {
    format!(
        "{}/tests/fixtures/fake-acp-agent-v2.mjs",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[tokio::test]
async fn acp_driver_v2_handles_upserts_correlation_and_foreground_state() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(vec![fixture_v2_path()], AcpProtocolPolicy::V2Only, true);
    let mut events = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start ACP v2 session");
    assert_eq!(session.metadata["protocol"]["version"], 2);

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "hello".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send v2 prompt");

    let mut collected = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("timed out")
            .expect("bus closed");
        let done = event.event_type == AgentEventType::TurnCompleted;
        collected.push(event);
        if done {
            break;
        }
    }

    let deltas: String = collected
        .iter()
        .filter(|event| event.event_type == AgentEventType::MessageDelta)
        .filter_map(|event| event.payload["text"].as_str())
        .collect();
    assert_eq!(deltas, "Hello v2");

    let completed = collected
        .iter()
        .find(|event| {
            event.event_type == AgentEventType::MessageCompleted
                && event.payload["message"]["role"] == "assistant"
        })
        .expect("completed assistant message");
    assert_eq!(completed.payload["message"]["content"], "Hello v2");

    assert!(collected
        .iter()
        .any(|event| event.event_type == AgentEventType::ToolStarted
            && event.payload["tool"]["id"] == "tool_v2"));
    assert!(collected
        .iter()
        .any(|event| event.event_type == AgentEventType::ToolCompleted
            && event.payload["tool"]["status"] == "completed"));
    assert!(collected
        .iter()
        .any(|event| event.event_type == AgentEventType::PlanUpdated));
    assert!(collected
        .iter()
        .any(|event| event.event_type == AgentEventType::ActivityChanged
            && event.payload["state"] == "running"));
    // Agent-owned terminals are a display event, never Fleet's v1 terminal API.
    assert!(collected.iter().any(|event| {
        event.event_type == AgentEventType::TerminalOutput
            && event.payload["terminal_id"] == "term_v2"
            && event.payload["text"] == "ok\n"
            && event.payload["done"] == true
            && event.payload["command"] == "cargo test"
    }));
    assert!(collected.iter().any(|event| {
        event.event_type == AgentEventType::TerminalOutput
            && event.payload["text"] == "more"
            && event.payload["done"] == false
    }));
    assert!(
        !collected.iter().any(|event| {
            event.event_type == AgentEventType::MessageStarted && event.payload["role"] == "user"
        }),
        "the echoed user message must be correlated, not rendered twice"
    );

    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_passes_session_mcp_servers_to_the_agent() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(
        vec![fixture_v2_path(), "--mcp".to_string()],
        AcpProtocolPolicy::V2Only,
        true,
    );
    let mut events = runtime.subscribe();
    let mut config = start_config(temp.path());
    config.extra = serde_json::json!({
        "mcp_servers": [{
            "type": "stdio",
            "name": "fleet",
            "command": "/usr/bin/true",
            "args": []
        }]
    });
    let session = runtime
        .start_session("machine_test", "test-acp", config)
        .await
        .expect("start");
    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "hello".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::MessageDelta
            && event.payload["text"] == "Hello v2 mcp=1"
    })
    .await;
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_v2_permission_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(
        vec![fixture_v2_path(), "--permission".to_string()],
        AcpProtocolPolicy::V2Only,
        true,
    );
    let mut events = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start");
    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "run".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("prompt");
    let permission_event = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::PermissionRequested
    })
    .await;
    let permission: AgentPermissionRequest =
        serde_json::from_value(permission_event.payload["permission"].clone()).unwrap();
    assert_eq!(permission.tool_call_id.as_deref(), Some("tool_v2"));
    runtime
        .respond_permission(PermissionDecision {
            permission_id: permission.id,
            session_id: Some(session.id.clone()),
            approve: true,
            option_id: Some("allow_once".to_string()),
            message: None,
        })
        .await
        .expect("respond");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
async fn acp_driver_process_crash_fails_the_open_turn() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(
        vec![fixture_v2_path(), "--crash".to_string()],
        AcpProtocolPolicy::V2Only,
        true,
    );
    let mut events = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start");
    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "crash".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("prompt");

    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnFailed
    })
    .await;
    wait_for_status(&runtime, &session.id, SessionStatus::Failed).await;
    // The instance remains recorded (failed), so diagnostics are available.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if runtime
            .instances()
            .iter()
            .any(|instance| instance.status.to_string() == "failed")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "instance did not become failed after the process crash"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn acp_driver_resumes_a_dead_process_when_advertised() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(
        vec![
            fixture_path(),
            "--resume".to_string(),
            "--crash".to_string(),
        ],
        AcpProtocolPolicy::V1Only,
        false,
    );
    let mut events = runtime.subscribe();
    let session = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect("start");

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "crash".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send crashing prompt");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::SessionError
    })
    .await;
    wait_for_status(&runtime, &session.id, SessionStatus::Failed).await;

    runtime
        .resume(&session.id)
        .await
        .expect("resume the dead ACP process");
    let resumed = runtime.get_session(&session.id).await.unwrap();
    assert_eq!(resumed.status, SessionStatus::Running);

    runtime
        .send_message(
            &session.id,
            SendMessageRequest {
                content: "hello after resume".to_string(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send after resume");
    let permission_event = next_matching(&mut events, |event| {
        event.event_type == AgentEventType::PermissionRequested
    })
    .await;
    let permission: AgentPermissionRequest =
        serde_json::from_value(permission_event.payload["permission"].clone()).unwrap();
    runtime
        .respond_permission(PermissionDecision {
            permission_id: permission.id,
            session_id: Some(session.id.clone()),
            approve: true,
            option_id: Some("allow_once".to_string()),
            message: None,
        })
        .await
        .expect("respond");
    next_matching(&mut events, |event| {
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    runtime.shutdown_session(&session.id).await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live `opencode acp` binary"]
async fn live_opencode_acp_probe_handshakes() {
    let mut launch = ProcessLaunchConfig::new("opencode");
    launch.args = vec!["acp".to_string()];
    let mut definition = AgentDefinition::process("opencode-acp", "OpenCode ACP", "acp", launch);
    definition.declared_capabilities = AgentCapabilities::full();
    let mut agents = AgentRegistry::new();
    agents.register(definition).expect("definition");
    let mut drivers = DriverRegistry::new();
    drivers
        .register(Arc::new(AcpDriver::new(AcpDriverConfig {
            protocol_policy: AcpProtocolPolicy::Auto,
            acp_v2_enabled: true,
            ..AcpDriverConfig::default()
        })))
        .expect("driver");
    let runtime = AgentRuntime::from_components(agents, drivers, 4096);
    let probe = runtime.probe_agent("opencode-acp").await.expect("probe");
    assert!(probe.available, "{probe:?}");
    assert!(probe.protocol_version.is_some(), "{probe:?}");
}

#[tokio::test]
async fn acp_driver_probe_reports_availability_and_capabilities() {
    let runtime = runtime_for(vec![fixture_path()], AcpProtocolPolicy::V1Only, false);
    let probe = runtime.probe_agent("test-acp").await.expect("probe");
    assert!(probe.available);
    assert_eq!(probe.driver_id, "acp");
    let effective = probe.capabilities.effective();
    assert!(effective.streaming);
    assert!(effective.tool_calls);
    assert!(effective.permission_approval);
}

#[tokio::test]
async fn acp_driver_v2_only_with_disabled_flag_fails_without_ghost_session() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime_for(vec![fixture_path()], AcpProtocolPolicy::V2Only, false);
    let error = runtime
        .start_session("machine_test", "test-acp", start_config(temp.path()))
        .await
        .expect_err("v2-only with the flag off must fail");
    assert!(matches!(
        error,
        fleet_daemon_runtime::RuntimeError::Unsupported(_)
    ));
    assert!(runtime.list_sessions().await.is_empty());
}
