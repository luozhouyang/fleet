//! End-to-end tests for the driver-based runtime with native drivers.
//!
//! These exercise the definition -> driver -> instance path, which the
//! legacy-provider constructor tests do not cover.

use std::sync::Arc;
use std::time::Duration;

use fleet_core_protocol::agent::{
    AgentCapabilities, AgentEvent, AgentEventType, AgentSession, AgentStartConfig,
    SendMessageRequest, SessionStatus,
};
use fleet_daemon_runtime::agent::ProcessLaunchConfig;
use fleet_daemon_runtime::MockProvider;
use fleet_daemon_runtime::{
    AgentDefinition, AgentRegistry, AgentRuntime, DriverRegistry, MockNativeDriver, RuntimeError,
};
use tokio::sync::broadcast;

fn mock_definition(id: &str, enabled: bool) -> AgentDefinition {
    let mut definition =
        AgentDefinition::process(id, "Mock Agent", "mock", ProcessLaunchConfig::new("mock"));
    definition.declared_capabilities = AgentCapabilities::full();
    definition.enabled = enabled;
    definition
}

fn runtime_with_driver_registry() -> AgentRuntime {
    let mut agents = AgentRegistry::new();
    agents
        .register(mock_definition("mock-agent", true))
        .expect("definition");
    agents
        .register(mock_definition("disabled-agent", false))
        .expect("definition");
    let mut drivers = DriverRegistry::new();
    drivers
        .register(Arc::new(MockNativeDriver::new(Arc::new(
            MockProvider::new(),
        ))))
        .expect("driver");
    AgentRuntime::from_components(agents, drivers, 1024)
}

fn start_config(delay_ms: u64) -> AgentStartConfig {
    AgentStartConfig {
        cwd: std::env::temp_dir().to_string_lossy().to_string(),
        title: Some("driver runtime test".to_string()),
        extra: serde_json::json!({"delay_ms": delay_ms}),
        ..AgentStartConfig::default()
    }
}

async fn next_matching(
    receiver: &mut broadcast::Receiver<AgentEvent>,
    predicate: impl Fn(&AgentEvent) -> bool,
) -> AgentEvent {
    let wait = async {
        loop {
            match receiver.recv().await {
                Ok(event) if predicate(&event) => return event,
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => panic!("event bus closed"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait)
        .await
        .expect("timed out waiting for event")
}

#[tokio::test]
async fn native_driver_completes_a_turn_and_tracks_the_instance() {
    let runtime = runtime_with_driver_registry();
    let mut events = runtime.subscribe();
    let session: AgentSession = runtime
        .start_session("machine_test", "mock-agent", start_config(1))
        .await
        .expect("start");
    assert_eq!(session.metadata["agent"]["definition_id"], "mock-agent");
    assert_eq!(session.metadata["agent"]["driver_id"], "mock");
    assert!(session.metadata["agent"]["instance_id"].is_string());
    assert_eq!(runtime.instances().len(), 1);
    assert_eq!(runtime.instances()[0].status.to_string(), "ready");

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
        event.event_type == AgentEventType::TurnCompleted
    })
    .await;
    // Finished work must not leave the instance permanently `busy`.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if runtime.instances()[0].status.to_string() == "ready" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "instance stayed busy after the turn completed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    runtime.shutdown_session(&session.id).await.unwrap();
    assert!(runtime.instances().is_empty());
    let session = runtime.get_session(&session.id).await.unwrap();
    assert_eq!(session.status, SessionStatus::Terminated);
}

#[tokio::test]
async fn one_driver_serves_multiple_definitions() {
    let mut agents = AgentRegistry::new();
    agents
        .register(mock_definition("mock-a", true))
        .expect("definition");
    agents
        .register(mock_definition("mock-b", true))
        .expect("definition");
    let mut drivers = DriverRegistry::new();
    drivers
        .register(Arc::new(MockNativeDriver::new(Arc::new(
            MockProvider::new(),
        ))))
        .expect("driver");
    let runtime = AgentRuntime::from_components(agents, drivers, 1024);

    let first = runtime
        .start_session("machine_test", "mock-a", start_config(1))
        .await
        .expect("first");
    let second = runtime
        .start_session("machine_test", "mock-b", start_config(1))
        .await
        .expect("second");
    assert_eq!(runtime.instances().len(), 2);
    assert_eq!(first.metadata["agent"]["definition_id"], "mock-a");
    assert_eq!(second.metadata["agent"]["definition_id"], "mock-b");
    runtime.shutdown_all().await;
}

#[tokio::test]
async fn disabled_definition_cannot_start_a_session() {
    let runtime = runtime_with_driver_registry();
    let error = runtime
        .start_session("machine_test", "disabled-agent", start_config(1))
        .await
        .expect_err("disabled definitions must not start");
    assert!(matches!(error, RuntimeError::AgentDisabled(_)));
    assert!(runtime.list_sessions().await.is_empty());
}

#[tokio::test]
async fn unknown_driver_is_reported_clearly() {
    let mut agents = AgentRegistry::new();
    let mut definition = mock_definition("broken", true);
    definition.driver_id = "missing-driver".to_string();
    agents.register(definition).unwrap();
    let runtime = AgentRuntime::from_components(agents, DriverRegistry::new(), 64);
    let error = runtime
        .start_session("machine_test", "broken", start_config(1))
        .await
        .expect_err("missing driver must fail");
    assert!(matches!(error, RuntimeError::DriverNotFound { .. }));
}

#[tokio::test]
async fn agent_and_driver_catalogs_are_exposed() {
    let runtime = runtime_with_driver_registry();
    let agents = runtime.agents();
    assert!(agents
        .iter()
        .any(|definition| definition.id == "mock-agent"));
    let drivers = runtime.drivers();
    assert!(drivers.iter().any(|driver| driver.id == "mock"));
    let providers = runtime.providers();
    let mock = providers
        .iter()
        .find(|provider| provider.id == "mock-agent")
        .expect("mock provider");
    // The native wrapper keeps the adapter's descriptor: availability,
    // version and models must survive the driver migration.
    assert!(mock.available);
    assert!(mock.version.is_some());
    assert_eq!(mock.models, vec!["mock-1".to_string()]);
}

#[tokio::test]
async fn unregistered_driver_is_not_advertised_as_available() {
    let mut agents = AgentRegistry::new();
    let mut definition = mock_definition("broken", true);
    definition.driver_id = "missing-driver".to_string();
    agents.register(definition).unwrap();
    let runtime = AgentRuntime::from_components(agents, DriverRegistry::new(), 64);
    let providers = runtime.providers();
    assert_eq!(providers.len(), 1);
    assert!(!providers[0].available);
}
