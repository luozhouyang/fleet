//! Build agent definitions from daemon configuration.
//!
//! Built-in definitions cover the adapters shipped with the daemon. The
//! `[agents.<id>]` table can override any built-in field or add a new agent
//! (for example an ACP-compatible agent installed on the machine).

use std::collections::BTreeMap;

use fleet_core_config::{AgentDefinitionConfig, DaemonConfig};
use fleet_core_protocol::agent::AgentCapabilities;
use serde_json::Value;

use crate::agent::definition::{
    AgentDefinition, AgentDefinitionSource, AgentLaunchConfig, CwdPolicy, ProcessLaunchConfig,
};

/// The config section that defines agents.
pub fn builtin_definitions(config: &DaemonConfig) -> Vec<AgentDefinition> {
    let mut definitions = vec![
        legacy_definition("mock", "Mock Agent", "mock", "mock", "mock"),
        legacy_definition(
            "claude",
            "Claude Code",
            "claude-native",
            "claude-code",
            &config.agent.claude_bin,
        ),
        acp_definition(
            "claude-acp",
            "Claude Code (ACP)",
            &config.agent.claude_acp_bin,
            Vec::new(),
        ),
        legacy_definition(
            "codex",
            "Codex",
            "codex-native",
            "codex",
            &config.agent.codex_bin,
        ),
        legacy_definition(
            "opencode",
            "OpenCode",
            "opencode-native",
            "opencode",
            &config.agent.opencode_bin,
        ),
    ];
    if let Ok(bin) = std::env::var("FLEET_GENERIC_BIN") {
        if !bin.is_empty() {
            let args = std::env::var("FLEET_GENERIC_ARGS")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>();
            let mut launch = ProcessLaunchConfig::new(bin);
            launch.args = args;
            let mut definition =
                AgentDefinition::process("generic", "Generic PTY", "generic", launch);
            definition.metadata = serde_json::json!({
                "provider_kind": "generic",
                "agent_type": "generic",
                "legacy_provider": true,
            });
            definitions.push(definition);
        }
    }
    definitions
}

fn legacy_definition(
    id: &str,
    name: &str,
    driver_id: &str,
    agent_type: &str,
    executable: &str,
) -> AgentDefinition {
    let mut definition =
        AgentDefinition::process(id, name, driver_id, ProcessLaunchConfig::new(executable));
    definition.source = AgentDefinitionSource::Builtin;
    definition.metadata = serde_json::json!({
        "provider_kind": id,
        "agent_type": agent_type,
        "legacy_provider": true,
    });
    definition
}

/// Build a built-in ACP agent definition.
///
/// The declared capabilities start permissive because ACP agents report their
/// real capabilities during `initialize`; the probe and the capability
/// intersection prevent unverified features from reaching the UI.
fn acp_definition(id: &str, name: &str, executable: &str, args: Vec<String>) -> AgentDefinition {
    let mut launch = ProcessLaunchConfig::new(executable);
    launch.args = args;
    let mut definition = AgentDefinition::process(id, name, "acp", launch);
    definition.source = AgentDefinitionSource::Builtin;
    definition.declared_capabilities = AgentCapabilities::full();
    definition.metadata = serde_json::json!({
        "provider_kind": "acp",
        "agent_type": id,
    });
    definition
}

/// Apply the `[agents]` table on top of the built-in definitions.
///
/// Returns definitions in id order. An entry for an existing id overrides
/// fields on that definition; an entry for a new id creates a definition whose
/// driver defaults to `acp`.
pub fn merge_user_definitions(
    builtins: Vec<AgentDefinition>,
    overrides: &BTreeMap<String, AgentDefinitionConfig>,
) -> Result<Vec<AgentDefinition>, String> {
    let mut by_id: BTreeMap<String, AgentDefinition> = builtins
        .into_iter()
        .map(|definition| (definition.id.clone(), definition))
        .collect();

    for (id, config) in overrides {
        let existing = by_id.get(id).cloned();
        let definition = merge_one(id, config, existing)?;
        by_id.insert(id.to_string(), definition);
    }

    Ok(by_id.into_values().collect())
}

fn merge_one(
    id: &str,
    config: &AgentDefinitionConfig,
    existing: Option<AgentDefinition>,
) -> Result<AgentDefinition, String> {
    let base = existing.unwrap_or_else(|| {
        let mut definition = AgentDefinition::process(
            id,
            config.name.clone().unwrap_or_else(|| id.to_string()),
            config.driver.clone().unwrap_or_else(|| "acp".to_string()),
            ProcessLaunchConfig::new(id),
        );
        definition.source = AgentDefinitionSource::UserConfig;
        definition.declared_capabilities = AgentCapabilities::full();
        definition
    });

    let mut definition = base;
    // An explicit override is user configuration even when the base definition
    // is built in.
    if definition.source == AgentDefinitionSource::Builtin
        && (config.name.is_some()
            || config.description.is_some()
            || config.driver.is_some()
            || config.enabled.is_some()
            || config.launch.is_some()
            || config.metadata.is_some())
    {
        definition.source = AgentDefinitionSource::UserConfig;
    }
    if let Some(name) = &config.name {
        definition.name = name.clone();
    }
    if let Some(description) = &config.description {
        definition.description = Some(description.clone());
    }
    if let Some(driver) = &config.driver {
        definition.driver_id = driver.clone();
    }
    if let Some(enabled) = config.enabled {
        definition.enabled = enabled;
    }
    if let Some(preferred) = &config.preferred_driver {
        definition.preferred_driver = Some(preferred.clone());
    }
    if let Some(fallback) = &config.fallback_driver {
        definition.fallback_driver = Some(fallback.clone());
    }
    if let Some(metadata) = &config.metadata {
        let incoming = toml_to_json(metadata);
        // User metadata extends built-in metadata so identity fields such as
        // `agent_type` and `provider_kind` survive an unrelated override.
        if let (Some(target), Some(incoming)) =
            (definition.metadata.as_object_mut(), incoming.as_object())
        {
            for (key, value) in incoming {
                target.insert(key.clone(), value.clone());
            }
        } else {
            definition.metadata = incoming;
        }
    }
    if let Some(launch) = &config.launch {
        if let AgentLaunchConfig::Process(process) = &mut definition.launch {
            if let Some(executable) = &launch.executable {
                process.executable = executable.clone();
            }
            if !launch.args.is_empty() {
                process.args = launch.args.clone();
            }
            process.env = launch
                .env
                .iter()
                .map(|(key, value)| {
                    expand_env(value)
                        .map(|value| (key.clone(), value))
                        .map_err(|error| format!("agent `{id}`: {error}"))
                })
                .collect::<Result<_, String>>()?;
            if let Some(policy) = &launch.cwd_policy {
                process.cwd_policy = match policy.as_str() {
                    "session" => CwdPolicy::Session,
                    "workspace_root" => CwdPolicy::WorkspaceRoot,
                    "fixed" => CwdPolicy::Fixed(launch.cwd.clone().ok_or_else(|| {
                        format!("agent `{id}` uses cwd_policy = \"fixed\" without a cwd path")
                    })?),
                    other => {
                        return Err(format!("agent `{id}` has unknown cwd_policy `{other}`"));
                    }
                };
            }
            if let Some(timeout) = launch.startup_timeout_ms {
                process.startup_timeout_ms = timeout.max(1);
            }
            if let Some(timeout) = launch.shutdown_timeout_ms {
                process.shutdown_timeout_ms = timeout.max(1);
            }
        }
    }
    Ok(definition)
}

/// Expand `${VAR}` references against the daemon process environment.
pub fn expand_env(value: &str) -> Result<String, String> {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!("unterminated variable reference in `{value}`"));
        };
        let name = &after[..end];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("invalid variable name `{name}` in `{value}`"));
        }
        let resolved =
            std::env::var(name).map_err(|_| format!("environment variable `{name}` is not set"))?;
        output.push_str(&resolved);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn toml_to_json(value: &toml::Value) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core_config::AgentLaunchSettings;

    #[test]
    fn user_entry_overrides_builtin_fields() {
        let config = DaemonConfig::default();
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "claude".to_string(),
            AgentDefinitionConfig {
                name: Some("My Claude".into()),
                enabled: Some(false),
                launch: Some(AgentLaunchSettings {
                    args: vec!["--flag".into()],
                    startup_timeout_ms: Some(20_000),
                    ..AgentLaunchSettings::default()
                }),
                ..AgentDefinitionConfig::default()
            },
        );
        let merged = merge_user_definitions(builtin_definitions(&config), &overrides).unwrap();
        let claude = merged.iter().find(|d| d.id == "claude").unwrap();
        assert_eq!(claude.name, "My Claude");
        assert!(!claude.enabled);
        let process = claude.launch.process().unwrap();
        assert_eq!(process.args, vec!["--flag".to_string()]);
        assert_eq!(process.startup_timeout_ms, 20_000);
    }

    #[test]
    fn builtin_agent_types_match_the_legacy_providers() {
        let config = DaemonConfig::default();
        let definitions = builtin_definitions(&config);
        for (id, agent_type) in [
            ("mock", "mock"),
            ("claude", "claude-code"),
            ("claude-acp", "claude-acp"),
            ("codex", "codex"),
            ("opencode", "opencode"),
        ] {
            let definition = definitions
                .iter()
                .find(|definition| definition.id == id)
                .unwrap_or_else(|| panic!("missing definition `{id}`"));
            assert_eq!(
                definition.metadata["agent_type"], agent_type,
                "definition `{id}` must keep the legacy session agent_type"
            );
        }
    }

    #[test]
    fn new_agent_defaults_to_acp_driver() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "openhands".to_string(),
            AgentDefinitionConfig {
                name: Some("OpenHands".into()),
                ..AgentDefinitionConfig::default()
            },
        );
        let merged = merge_user_definitions(Vec::new(), &overrides).unwrap();
        assert_eq!(merged[0].driver_id, "acp");
        assert_eq!(merged[0].launch.process().unwrap().executable, "openhands");
    }

    #[test]
    fn invalid_launch_settings_fail_merge() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "agent".to_string(),
            AgentDefinitionConfig {
                launch: Some(AgentLaunchSettings {
                    cwd_policy: Some("fixed".into()),
                    ..AgentLaunchSettings::default()
                }),
                ..AgentDefinitionConfig::default()
            },
        );
        assert!(merge_user_definitions(Vec::new(), &overrides).is_err());

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "agent".to_string(),
            AgentDefinitionConfig {
                launch: Some(AgentLaunchSettings {
                    env: BTreeMap::from([(
                        "TOKEN".to_string(),
                        "${FLEET_MISSING_ENV}".to_string(),
                    )]),
                    ..AgentLaunchSettings::default()
                }),
                ..AgentDefinitionConfig::default()
            },
        );
        assert!(merge_user_definitions(Vec::new(), &overrides).is_err());
    }

    #[test]
    fn env_expansion_rejects_missing_variables() {
        assert_eq!(expand_env("plain").unwrap(), "plain");
        std::env::set_var("FLEET_TEST_AGENT_ENV", "value");
        assert_eq!(expand_env("a${FLEET_TEST_AGENT_ENV}b").unwrap(), "avalueb");
        assert!(expand_env("${FLEET_TEST_AGENT_ENV_MISSING}").is_err());
        assert!(expand_env("${unterminated").is_err());
    }
}
