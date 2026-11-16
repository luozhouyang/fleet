//! Credential storage: OS keyring on macOS and Windows, a 0600 file elsewhere.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{home_dir, write_private};

#[cfg(any(target_os = "macos", target_os = "windows"))]
const SERVICE: &str = "fleet";

/// Linux uses the file backend so release binaries stay free of libdbus and
/// can be linked statically against musl.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn keyring_entry(name: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, name).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn keyring_entry(_name: &str) -> Option<keyring::Entry> {
    None
}

pub const CLOUDFLARE_API_TOKEN: &str = "cloudflare-api-token";

#[derive(Debug, Default, Serialize, Deserialize)]
struct SecretFile {
    #[serde(default)]
    secrets: std::collections::BTreeMap<String, String>,
}

/// Load a secret from the OS keyring, then the fallback file.
pub fn load(name: &str) -> Option<String> {
    if let Some(entry) = keyring_entry(name) {
        if let Ok(value) = entry.get_password() {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    load_file(&fallback_path(), name)
}

/// Store a secret. Returns `true` when the OS keyring accepted it; otherwise
/// the secret is written to the 0600 fallback file.
pub fn store(name: &str, value: &str) -> Result<bool> {
    if let Some(entry) = keyring_entry(name) {
        if entry.set_password(value).is_ok() {
            return Ok(true);
        }
    }
    store_file(&fallback_path(), name, value)?;
    Ok(false)
}

pub fn fallback_path() -> PathBuf {
    if let Ok(path) = std::env::var("FLEET_SECRETS_FILE") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    home_dir().join(".fleet").join("secrets.toml")
}

fn load_file(path: &Path, name: &str) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let file: SecretFile = toml::from_str(&raw).ok()?;
    file.secrets.get(name).cloned()
}

fn store_file(path: &Path, name: &str, value: &str) -> Result<()> {
    let mut file = if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("invalid secrets file {}", path.display()))?
    } else {
        SecretFile::default()
    };
    file.secrets.insert(name.to_string(), value.to_string());
    let rendered = toml::to_string_pretty(&file).context("failed to serialize secrets")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    write_private(path, rendered.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_fallback_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.toml");
        store_file(&path, CLOUDFLARE_API_TOKEN, "token-123").unwrap();
        assert_eq!(
            load_file(&path, CLOUDFLARE_API_TOKEN).as_deref(),
            Some("token-123")
        );
        store_file(&path, "other", "value").unwrap();
        assert_eq!(
            load_file(&path, CLOUDFLARE_API_TOKEN).as_deref(),
            Some("token-123")
        );
        assert_eq!(load_file(&path, "other").as_deref(), Some("value"));
    }
}
