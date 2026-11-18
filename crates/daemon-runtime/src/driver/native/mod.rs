//! Native protocol driver wrappers.
//!
//! Each wrapper pins a [`crate::driver::legacy::LegacyProviderDriver`] to a
//! stable native driver id (`claude-native`, `codex-native`,
//! `opencode-native`, `generic`, `mock`). They let driver selection resolve
//! vendor protocols by id while the underlying adapters still implement
//! [`crate::provider::AgentProvider`].
//!
//! The wrappers add no behavior: every [`crate::driver::trait_def::AgentDriver`]
//! method is delegated to the wrapped legacy driver.

/// Defines a named native driver wrapper around a legacy provider driver.
///
/// The generated struct is a public tuple struct with the same API for every
/// driver: `new`, `into_inner`, `provider` and a full [`AgentDriver`]
/// implementation that delegates to the inner driver.
macro_rules! define_native_driver {
    (
        $(#[$struct_doc:meta])*
        $name:ident => $driver_id:literal
    ) => {
        $(#[$struct_doc])*
        pub struct $name($crate::driver::legacy::LegacyProviderDriver);

        impl $name {
            #[doc = concat!(
                "Wrap `provider` as the `",
                $driver_id,
                "` driver."
            )]
            pub fn new(
                provider: std::sync::Arc<dyn $crate::provider::AgentProvider>,
            ) -> Self {
                Self($crate::driver::legacy::LegacyProviderDriver::with_id(
                    provider,
                    $driver_id,
                ))
            }

            /// Consume the wrapper and return the underlying legacy driver.
            pub fn into_inner(self) -> $crate::driver::legacy::LegacyProviderDriver {
                self.0
            }

            /// The wrapped provider.
            pub fn provider(&self) -> &std::sync::Arc<dyn $crate::provider::AgentProvider> {
                self.0.provider()
            }
        }

        #[async_trait::async_trait]
        impl $crate::driver::trait_def::AgentDriver for $name {
            fn driver_id(&self) -> &str {
                $crate::driver::trait_def::AgentDriver::driver_id(&self.0)
            }

            fn info(&self) -> $crate::driver::trait_def::DriverInfo {
                $crate::driver::trait_def::AgentDriver::info(&self.0)
            }

            fn capabilities(&self) -> fleet_core_protocol::agent::AgentCapabilities {
                $crate::driver::trait_def::AgentDriver::capabilities(&self.0)
            }

            fn provider_info(
                &self,
            ) -> Option<fleet_core_protocol::agent::ProviderInfo> {
                $crate::driver::trait_def::AgentDriver::provider_info(&self.0)
            }

            async fn probe(
                &self,
                definition: &$crate::agent::AgentDefinition,
            ) -> $crate::error::Result<$crate::driver::trait_def::DriverProbeResult> {
                $crate::driver::trait_def::AgentDriver::probe(&self.0, definition).await
            }

            async fn start(
                &self,
                ctx: $crate::driver::context::DriverContext,
                definition: &$crate::agent::AgentDefinition,
                config: fleet_core_protocol::agent::AgentStartConfig,
            ) -> $crate::error::Result<$crate::driver::instance::DriverStartResult> {
                $crate::driver::trait_def::AgentDriver::start(&self.0, ctx, definition, config)
                    .await
            }

            async fn send_message(
                &self,
                instance: &$crate::driver::instance::AgentInstanceHandle,
                message: $crate::driver::trait_def::AgentPrompt,
            ) -> $crate::error::Result<$crate::driver::trait_def::PromptAcceptance> {
                $crate::driver::trait_def::AgentDriver::send_message(&self.0, instance, message)
                    .await
            }

            async fn interrupt(
                &self,
                instance: &$crate::driver::instance::AgentInstanceHandle,
            ) -> $crate::error::Result<()> {
                $crate::driver::trait_def::AgentDriver::interrupt(&self.0, instance).await
            }

            async fn resume(
                &self,
                instance: &$crate::driver::instance::AgentInstanceHandle,
                session: &fleet_core_protocol::agent::AgentSession,
            ) -> $crate::error::Result<$crate::driver::trait_def::ResumeResult> {
                $crate::driver::trait_def::AgentDriver::resume(&self.0, instance, session).await
            }

            async fn shutdown(
                &self,
                instance: &$crate::driver::instance::AgentInstanceHandle,
            ) -> $crate::error::Result<()> {
                $crate::driver::trait_def::AgentDriver::shutdown(&self.0, instance).await
            }

            async fn respond_permission(
                &self,
                instance: &$crate::driver::instance::AgentInstanceHandle,
                decision: fleet_core_protocol::agent::PermissionDecision,
            ) -> $crate::error::Result<()> {
                $crate::driver::trait_def::AgentDriver::respond_permission(
                    &self.0, instance, decision,
                )
                .await
            }

            async fn respond_question(
                &self,
                instance: &$crate::driver::instance::AgentInstanceHandle,
                decision: fleet_core_protocol::agent::QuestionDecision,
            ) -> $crate::error::Result<()> {
                $crate::driver::trait_def::AgentDriver::respond_question(
                    &self.0, instance, decision,
                )
                .await
            }
        }
    };
}

mod claude;
mod codex;
mod generic;
mod mock;
mod opencode;

pub use claude::ClaudeNativeDriver;
pub use codex::CodexNativeDriver;
pub use generic::GenericNativeDriver;
pub use mock::MockNativeDriver;
pub use opencode::OpenCodeNativeDriver;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use fleet_core_protocol::agent::{
        AgentCapabilities, AgentSession, AgentStartConfig, PermissionDecision, ProviderInfo,
        ProviderKind, QuestionDecision,
    };

    use crate::driver::trait_def::{AgentDriver, DriverProtocol};
    use crate::error::Result;
    use crate::provider::{AgentProvider, ProviderContext};

    use super::{
        ClaudeNativeDriver, CodexNativeDriver, GenericNativeDriver, MockNativeDriver,
        OpenCodeNativeDriver,
    };

    /// A provider whose only observable behavior is its descriptor.
    struct StubProvider {
        id: &'static str,
    }

    #[async_trait]
    impl AgentProvider for StubProvider {
        fn provider_id(&self) -> &'static str {
            self.id
        }

        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                id: self.id.to_string(),
                kind: ProviderKind::Mock,
                display_name: format!("stub {}", self.id),
                version: None,
                available: true,
                capabilities: self.capabilities(),
                models: vec![],
            }
        }

        fn capabilities(&self) -> AgentCapabilities {
            AgentCapabilities::basic()
        }

        async fn start(
            &self,
            _ctx: ProviderContext,
            _config: AgentStartConfig,
        ) -> Result<AgentSession> {
            unimplemented!("stub provider does not start sessions")
        }

        async fn send_message(&self, _session_id: &str, _content: String) -> Result<()> {
            Ok(())
        }

        async fn interrupt(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }

        async fn resume(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }

        async fn shutdown(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }

        async fn respond_permission(
            &self,
            _session_id: &str,
            _decision: PermissionDecision,
        ) -> Result<()> {
            Ok(())
        }

        async fn respond_question(
            &self,
            _session_id: &str,
            _decision: QuestionDecision,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn stub(id: &'static str) -> Arc<StubProvider> {
        Arc::new(StubProvider { id })
    }

    fn assert_native(driver: &dyn AgentDriver, expected_id: &str) {
        assert_eq!(driver.driver_id(), expected_id);
        let info = driver.info();
        assert_eq!(info.id, expected_id);
        assert_eq!(info.protocol, DriverProtocol::Native);
        assert_eq!(driver.capabilities(), AgentCapabilities::basic());
    }

    #[test]
    fn claude_native_driver_reports_native_protocol() {
        assert_native(&ClaudeNativeDriver::new(stub("claude")), "claude-native");
    }

    #[test]
    fn codex_native_driver_reports_native_protocol() {
        assert_native(&CodexNativeDriver::new(stub("codex")), "codex-native");
    }

    #[test]
    fn opencode_native_driver_reports_native_protocol() {
        assert_native(
            &OpenCodeNativeDriver::new(stub("opencode")),
            "opencode-native",
        );
    }

    #[test]
    fn generic_native_driver_reports_native_protocol() {
        assert_native(&GenericNativeDriver::new(stub("generic")), "generic");
    }

    #[test]
    fn mock_native_driver_reports_native_protocol() {
        assert_native(&MockNativeDriver::new(stub("mock")), "mock");
    }

    #[test]
    fn native_drivers_expose_the_wrapped_provider() {
        let driver = ClaudeNativeDriver::new(stub("claude"));
        assert_eq!(driver.provider().provider_id(), "claude");
        assert_eq!(driver.into_inner().driver_id(), "claude-native");
    }
}
