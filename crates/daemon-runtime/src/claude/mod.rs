//! Claude Code CLI adapter.
//!
//! The adapter speaks the documented `--input-format stream-json` /
//! `--output-format stream-json` protocol. Native types live in
//! [`protocol`], mapping to unified events lives in [`mapper`], and process and
//! session orchestration lives in [`adapter`].

mod adapter;
mod mapper;
#[allow(dead_code)]
mod protocol;

pub use adapter::ClaudeProvider;

#[cfg(test)]
mod tests {
    use super::adapter::ClaudeProvider;
    use crate::provider::AgentProvider;

    #[test]
    fn capabilities_are_honest() {
        let provider = ClaudeProvider::new("claude");
        let capabilities = provider.capabilities();
        assert!(capabilities.streaming);
        assert!(capabilities.interrupt);
        assert!(capabilities.resume);
        assert!(!capabilities.permission_approval);
        assert!(capabilities.structured_events);
        assert!(!capabilities.structured_diff);
        assert!(!capabilities.images);
        assert_eq!(provider.agent_type(), "claude-code");
    }

    #[test]
    fn availability_reflects_the_binary() {
        assert!(ClaudeProvider::new("sh").info().available);
        assert!(
            !ClaudeProvider::new("definitely-not-a-real-claude-binary")
                .info()
                .available
        );
    }

    #[test]
    fn resume_can_be_disabled() {
        let provider = ClaudeProvider::with_resume_support("claude", false);
        assert!(!provider.capabilities().resume);
        assert!(!provider.resume_supported());
    }

    #[tokio::test]
    async fn resume_is_unsupported_without_spawning() {
        let provider = ClaudeProvider::with_resume_support("definitely-not-claude", false);
        let error = provider
            .resume("missing")
            .await
            .expect_err("resume must be unsupported");
        assert!(matches!(error, crate::RuntimeError::Unsupported(_)));
    }
}
