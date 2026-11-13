//! Codex app-server adapter.
//!
//! The adapter launches `codex app-server --listen stdio://` and speaks
//! JSON-RPC 2.0 over JSONL. See `fixtures/README.md` for the exact methods and
//! the schema version they were verified against.

mod adapter;
mod mapper;
#[allow(dead_code)]
mod protocol;

pub use adapter::CodexProvider;

#[cfg(test)]
mod tests {
    use super::CodexProvider;
    use crate::provider::AgentProvider;

    #[tokio::test]
    async fn interrupt_unknown_session_does_not_spawn() {
        let provider = CodexProvider::new("definitely-not-codex");
        let error = provider
            .interrupt("missing")
            .await
            .expect_err("missing sessions must fail");
        assert!(matches!(error, crate::RuntimeError::SessionNotFound(_)));
    }

    #[test]
    fn capabilities_are_honest() {
        let provider = CodexProvider::new("codex");
        let capabilities = provider.capabilities();
        assert!(capabilities.streaming);
        assert!(capabilities.permission_approval);
        assert!(capabilities.resume);
        assert!(capabilities.tool_calls);
        assert!(!capabilities.structured_diff);
        assert!(!capabilities.images);
    }
}
