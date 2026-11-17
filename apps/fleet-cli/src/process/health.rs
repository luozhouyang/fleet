//! Health probes derived from each role's configuration.

use std::time::Duration;

use anyhow::{Context, Result};

use crate::config_paths::{Role, RolePaths};

/// Build the local health URL for `paths`.
///
/// Wildcard binds are probed through loopback because connecting to
/// `0.0.0.0` or `::` is not portable.
pub fn health_url(paths: &RolePaths) -> Result<String> {
    let authority = match paths.role {
        Role::Admin => {
            let config = fleet_core_config::AdminConfig::load(Some(&paths.config))
                .with_context(|| format!("invalid admin config {}", paths.config.display()))?;
            authority(&config.server.host, config.server.port)
        }
        Role::Daemon => {
            let config = fleet_core_config::DaemonConfig::load(Some(&paths.config))
                .with_context(|| format!("invalid daemon config {}", paths.config.display()))?;
            authority(&config.daemon.host, config.daemon.port)
        }
    };
    let suffix = match paths.role {
        Role::Admin => "/api/v1/health",
        Role::Daemon => "/health",
    };
    Ok(format!("http://{authority}{suffix}"))
}

fn authority(host: &str, port: u16) -> String {
    let host = match host {
        "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        other => other,
    };
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Probe `url` once. Any non-2xx response, timeout, or connection error is
/// reported as unhealthy.
pub async fn probe_url(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    match client.get(url).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

/// Fetch the health payload when the endpoint is reachable and returns JSON.
pub async fn probe_json(url: &str) -> Option<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

/// Probe the role's local health endpoint.
pub async fn probe(paths: &RolePaths) -> bool {
    match health_url(paths) {
        Ok(url) => probe_url(&url).await,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_paths::RolePaths;
    use std::path::{Path, PathBuf};

    fn paths(dir: &Path, role: Role, config: PathBuf) -> RolePaths {
        RolePaths {
            role,
            config,
            state_dir: dir.join("state"),
            database: dir.join("state/db.sqlite"),
            binary: None,
            fleet_binary: PathBuf::from("/usr/local/bin/fleet"),
            service_user: "tester".to_string(),
            service_group: "tester".to_string(),
            web_dir: None,
        }
    }

    #[test]
    fn admin_url_uses_config_and_loopback_for_wildcard_binds() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("admin.toml");
        std::fs::write(&config, "[server]\nhost = \"0.0.0.0\"\nport = 9911\n").unwrap();
        let paths = paths(dir.path(), Role::Admin, config);
        assert_eq!(
            health_url(&paths).unwrap(),
            "http://127.0.0.1:9911/api/v1/health"
        );
    }

    #[test]
    fn daemon_url_uses_the_health_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("daemon.toml");
        std::fs::write(&config, "[daemon]\nhost = \"::1\"\nport = 18081\n").unwrap();
        let paths = paths(dir.path(), Role::Daemon, config);
        assert_eq!(health_url(&paths).unwrap(), "http://[::1]:18081/health");
    }
}
