//! Map ACP v1 and v2 agent capabilities into the Fleet capability model.
//!
//! v1 and v2 organize capabilities differently, so the two mappings are
//! separate. Only capabilities Fleet actually implements are reported as
//! supported; anything else is `Unsupported` and removed by the capability
//! intersection.

use fleet_core_protocol::agent::AgentCapabilities;

use super::config::AcpProtocolVersion;

/// Map an ACP v1 `initialize` response's capabilities.
pub fn map_v1(agent: &agent_client_protocol::v1::schema::AgentCapabilities) -> AgentCapabilities {
    let prompt = &agent.prompt_capabilities;
    let sessions = &agent.session_capabilities;
    // `load_session` covers `session/load`, not `session/resume`. The driver's
    // recovery path sends `session/resume`, so only that capability counts.
    let resume_advertised = sessions.resume.is_some();
    let persistence = agent.load_session || sessions.resume.is_some();
    AgentCapabilities {
        streaming: true,
        interrupt: true,
        resume: resume_advertised,
        permission_approval: true,
        user_questions: true,
        structured_events: true,
        structured_diff: false,
        session_persistence: persistence,
        tool_calls: true,
        images: prompt.image,
    }
}

/// Map an ACP v2 `initialize` response's capabilities.
pub fn map_v2(agent: &agent_client_protocol::v2::schema::AgentCapabilities) -> AgentCapabilities {
    let session_advertised = agent.session.is_some();
    let images = agent
        .session
        .as_ref()
        .and_then(|session| session.prompt.as_ref())
        .and_then(|prompt| prompt.image.as_ref())
        .is_some();
    AgentCapabilities {
        streaming: true,
        interrupt: true,
        // v2 baseline session methods include `session/resume`.
        resume: session_advertised,
        permission_approval: true,
        user_questions: true,
        structured_events: true,
        structured_diff: false,
        session_persistence: session_advertised,
        tool_calls: true,
        images,
    }
}

/// Map capabilities for the negotiated protocol version.
pub fn map_for(
    version: AcpProtocolVersion,
    response: &AcpInitializeCapabilities<'_>,
) -> AgentCapabilities {
    match (version, response) {
        (AcpProtocolVersion::V1, AcpInitializeCapabilities::V1(capabilities)) => {
            map_v1(capabilities)
        }
        (AcpProtocolVersion::V2, AcpInitializeCapabilities::V2(capabilities)) => {
            map_v2(capabilities)
        }
        // A mismatched variant means the caller mislabeled the connection.
        _ => AgentCapabilities::default(),
    }
}

/// Borrowed initialize-time capabilities from either protocol version.
pub enum AcpInitializeCapabilities<'a> {
    /// ACP v1 agent capabilities.
    V1(&'a agent_client_protocol::v1::schema::AgentCapabilities),
    /// ACP v2 agent capabilities.
    V2(&'a agent_client_protocol::v2::schema::AgentCapabilities),
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema as v1;
    use agent_client_protocol::v2::schema as v2;

    #[test]
    fn v1_images_follow_prompt_capabilities() {
        let mut capabilities = v1::AgentCapabilities::new();
        capabilities.prompt_capabilities.image = true;
        let mapped = map_v1(&capabilities);
        assert!(mapped.streaming);
        assert!(mapped.images);
        assert!(!mapped.resume);
        assert!(!mapped.session_persistence);
    }

    #[test]
    fn v1_resume_requires_advertised_session_support() {
        let mut capabilities = v1::AgentCapabilities::new();
        capabilities.session_capabilities.resume = Some(Default::default());
        let mapped = map_v1(&capabilities);
        assert!(mapped.resume);
        assert!(mapped.session_persistence);
    }

    #[test]
    fn v1_load_session_does_not_imply_session_resume() {
        let mut capabilities = v1::AgentCapabilities::new();
        capabilities.load_session = true;
        let mapped = map_v1(&capabilities);
        assert!(!mapped.resume);
        assert!(mapped.session_persistence);
    }

    #[test]
    fn v2_session_capability_enables_baseline_features() {
        let mut capabilities = v2::AgentCapabilities::new();
        capabilities.session = Some(v2::SessionCapabilities::new());
        let mapped = map_v2(&capabilities);
        assert!(mapped.resume);
        assert!(mapped.session_persistence);
        assert!(!mapped.images);
    }
}
