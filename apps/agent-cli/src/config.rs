use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{value, DocumentMut, Item, Table};

pub const DEFAULT_ADMIN_URL: &str = "http://127.0.0.1:8787";
const CLI_CONFIG_ENV: &str = "AGENT_FLEET_CLI_CONFIG";

/// Client-side settings for `af`. Service configuration lives in the admin and
/// daemon TOML files and is edited surgically by `af network wizard`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CliConfig {
    /// Admin base URL used by the CLI (usually loopback).
    pub admin_url: Option<String>,
    /// Bearer token. Prefer `AGENT_FLEET_ADMIN_TOKEN` in shared environments.
    pub token: Option<String>,
    /// `table` or `json`.
    pub output: Option<String>,
    /// Public URL recorded by the network wizard for reference.
    pub public_url: Option<String>,
}

impl CliConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("invalid CLI config {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let raw = toml::to_string_pretty(self).context("failed to serialize CLI config")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        write_private(path, raw.as_bytes())
    }
}

/// `~/.agent-fleet/cli.toml`, overridable with `AGENT_FLEET_CLI_CONFIG`.
pub fn default_cli_config_path() -> PathBuf {
    if let Ok(path) = std::env::var(CLI_CONFIG_ENV) {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    home_dir().join(".agent-fleet").join("cli.toml")
}

/// `~/.agent-fleet/config.toml`, shared by the admin and daemon defaults.
pub fn default_service_config_path() -> PathBuf {
    home_dir().join(".agent-fleet").join("config.toml")
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// One surgical TOML edit: `[section] key = "value"`.
#[derive(Debug, Clone)]
pub struct TomlEdit {
    pub section: &'static str,
    pub key: &'static str,
    pub value: String,
}

/// Update selected keys in a TOML file, preserving comments and unrelated
/// fields. The previous file is copied to `<path>.bak` (or `.bak.<ts>` if a
/// backup already exists).
pub fn edit_toml(path: &Path, edits: &[TomlEdit]) -> Result<Option<PathBuf>> {
    let backup = if path.exists() {
        Some(backup_file(path)?)
    } else {
        None
    };

    let mut document = if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        raw.parse::<DocumentMut>()
            .with_context(|| format!("invalid TOML in {}", path.display()))?
    } else {
        DocumentMut::new()
    };

    for edit in edits {
        let entry = document
            .entry(edit.section)
            .or_insert(Item::Table(Table::new()));
        entry[edit.key] = value(edit.value.clone());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    write_private(path, document.to_string().as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(backup)
}

/// Copy `path` next to itself. Existing backups are never overwritten.
pub fn backup_file(path: &Path) -> Result<PathBuf> {
    let base = PathBuf::from(format!("{}.bak", path.display()));
    if !base.exists() {
        fs::copy(path, &base).with_context(|| format!("failed to back up {}", path.display()))?;
        return Ok(base);
    }
    let stamp = chrono::Local::now().format("%Y%m%d%H%M%S");
    let mut candidate = PathBuf::from(format!("{}.bak.{stamp}", path.display()));
    let mut suffix = 1u32;
    while candidate.exists() {
        candidate = PathBuf::from(format!("{}.bak.{stamp}.{suffix}", path.display()));
        suffix += 1;
    }
    fs::copy(path, &candidate).with_context(|| format!("failed to back up {}", path.display()))?;
    Ok(candidate)
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_preserve_comments_and_create_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "# keep me\n[server]\nhost = \"127.0.0.1\"\npublic_url = \"https://old\"\n",
        )
        .unwrap();

        let backup = edit_toml(
            &path,
            &[TomlEdit {
                section: "server",
                key: "public_url",
                value: "https://new.example.com".into(),
            }],
        )
        .unwrap()
        .expect("backup");
        assert!(backup.exists());

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("host = \"127.0.0.1\""));
        assert!(raw.contains("public_url = \"https://new.example.com\""));
    }

    #[test]
    fn creates_missing_files_and_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");
        let backup = edit_toml(
            &path,
            &[TomlEdit {
                section: "connection",
                key: "server_url",
                value: "wss://fleet.example.com/ws/machine".into(),
            }],
        )
        .unwrap();
        assert!(backup.is_none());
        let parsed: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            parsed["connection"]["server_url"].as_str(),
            Some("wss://fleet.example.com/ws/machine")
        );
    }

    #[test]
    fn cli_config_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli.toml");
        let config = CliConfig {
            admin_url: Some(DEFAULT_ADMIN_URL.into()),
            token: Some("secret".into()),
            output: Some("json".into()),
            public_url: Some("https://fleet.example.com".into()),
        };
        config.save(&path).unwrap();
        let loaded = CliConfig::load(&path).unwrap();
        assert_eq!(loaded.admin_url.as_deref(), Some(DEFAULT_ADMIN_URL));
        assert_eq!(loaded.token.as_deref(), Some("secret"));
        assert_eq!(loaded.output.as_deref(), Some("json"));
        assert_eq!(
            loaded.public_url.as_deref(),
            Some("https://fleet.example.com")
        );
    }
}
