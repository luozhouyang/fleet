use std::path::PathBuf;

use fleet_core_auth::{
    load_or_create_machine_id, machine_token_path, save_machine_token, MachineIdentity,
};
use fleet_core_config::DaemonConfig;
use fleet_core_connection::{pair_machine, PairingConfig, PairingResult};
use fleet_core_protocol::workspace::RegisterWorkspaceRequest;
use fleet_core_protocol::PROTOCOL_VERSION;

use crate::services::Services;

/// Register a workspace from the CLI and print the result as JSON.
pub async fn register_workspace(
    config_path: Option<PathBuf>,
    root: PathBuf,
    name: Option<String>,
) -> anyhow::Result<()> {
    let config = DaemonConfig::load(config_path.as_deref())?;
    let services = Services::build_local(config).await?;
    let workspace = services
        .workspaces
        .register(
            &services.machine_id,
            RegisterWorkspaceRequest {
                root_path: root.to_string_lossy().to_string(),
                name,
                project_id: None,
            },
        )
        .await?;
    println!("{}", serde_json::to_string_pretty(&workspace)?);
    Ok(())
}

/// Print the provider registry as JSON.
pub async fn print_providers(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = DaemonConfig::load(config_path.as_deref())?;
    let services = Services::build_local(config).await?;
    println!("{}", serde_json::to_string_pretty(&services.providers())?);
    Ok(())
}

/// List registered local workspaces as JSON.
pub async fn list_workspaces(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = DaemonConfig::load(config_path.as_deref())?;
    let services = Services::build_local(config).await?;
    let workspaces = services.workspaces.list(Some(&services.machine_id)).await?;
    println!("{}", serde_json::to_string_pretty(&workspaces)?);
    Ok(())
}

/// Remove a registered local workspace.
///
/// Removal is destructive, so it refuses to run without `--yes`.
pub async fn remove_workspace(
    config_path: Option<PathBuf>,
    id: String,
    yes: bool,
) -> anyhow::Result<()> {
    if !yes {
        anyhow::bail!("removing a workspace requires --yes");
    }
    let config = DaemonConfig::load(config_path.as_deref())?;
    let services = Services::build_local(config).await?;
    services.workspaces.remove(&id).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "removed": id }))?
    );
    Ok(())
}

/// Pair this machine with the agent admin and save the per-machine token.
///
/// Only the token is written; the daemon config is not modified. Use `fleet daemon setup`
/// to point the config at the admin.
pub async fn pair(
    config_path: Option<PathBuf>,
    server: String,
    code: String,
    name: Option<String>,
) -> anyhow::Result<()> {
    let mut config = DaemonConfig::load(config_path.as_deref())?;
    config.resolve_paths();

    let identity = MachineIdentity::load_or_create(&config.machine.key_file)?;
    let previous = MachineIdentity::load_previous(&config.machine.key_file)?;
    let machine_id = match config.machine.id.clone() {
        Some(id) => id,
        None => load_or_create_machine_id(&config.machine.key_file, &identity)?,
    };
    let machine_name = name
        .or_else(|| config.machine.name.clone())
        .unwrap_or_else(|| machine_id.clone());
    let hostname = fleet_core_connection::local_hostname();

    let result = pair_machine(
        PairingConfig {
            server_url: server,
            machine_id: machine_id.clone(),
            machine_name,
            hostname,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: PROTOCOL_VERSION,
            pairing_code: code,
            handshake_timeout: std::time::Duration::from_secs(10),
        },
        &identity,
        previous.as_ref(),
    )
    .await?;

    let token_path = machine_token_path(&config.machine.key_file);
    save_machine_token(&token_path, &result.token)?;

    println!(
        "{}",
        serde_json::to_string_pretty(&pair_summary(&result, &token_path))?
    );
    Ok(())
}

/// Printable summary. It deliberately omits the token itself.
fn pair_summary(result: &PairingResult, token_path: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "machine_id": result.machine_id,
        "org_id": result.org_id,
        "token_file": token_path,
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use fleet_core_common::{now, ApiError};
    use fleet_core_connection::{ConnectionGateway, MachineAuthenticator};
    use fleet_core_protocol::pairing::{
        MachineAuthState, PairingCodeInfo, PairingCodeStatus, PairingCompletion, PairingOutcome,
    };
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    const CODE: &str = "ABCD-EFGH-JKMN";

    struct PairingOnlyAuth;

    #[async_trait]
    impl MachineAuthenticator for PairingOnlyAuth {
        async fn machine_state(&self, _machine_id: &str) -> Result<MachineAuthState, ApiError> {
            Ok(MachineAuthState { pinned_key: None })
        }

        async fn pin_key(&self, _machine_id: &str, _public_key: &str) -> Result<(), ApiError> {
            Ok(())
        }

        async fn is_revoked(&self, _machine_id: &str) -> Result<bool, ApiError> {
            Ok(false)
        }

        async fn machine_for_token(&self, _token: &str) -> Result<Option<String>, ApiError> {
            Ok(None)
        }

        async fn pairing_code(&self, code: &str) -> Result<PairingCodeStatus, ApiError> {
            if code == CODE {
                Ok(PairingCodeStatus::Valid(PairingCodeInfo {
                    id: "pair_1".into(),
                    org_id: "org_default".into(),
                    created_by: "admin".into(),
                    machine_name_hint: None,
                    expires_at: now() + chrono::Duration::seconds(60),
                    attempts_left: 5,
                }))
            } else {
                Ok(PairingCodeStatus::Invalid)
            }
        }

        async fn record_pairing_failure(&self, _code_id: &str) -> Result<(), ApiError> {
            Ok(())
        }

        async fn complete_pairing(
            &self,
            request: PairingCompletion,
        ) -> Result<PairingOutcome, ApiError> {
            Ok(PairingOutcome {
                machine_id: request.machine_id,
                org_id: "org_default".into(),
                paired_at: now(),
            })
        }
    }

    async fn start_gateway() -> String {
        let gateway = ConnectionGateway::new(
            Arc::new(PairingOnlyAuth),
            Duration::from_millis(500),
            Duration::from_secs(5),
        );
        let app = gateway.router();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("ws://{addr}/machine")
    }

    fn write_config(dir: &Path) -> PathBuf {
        let key_file = dir.join("machine.key");
        let database = dir.join("fleet-daemon.db");
        let path = dir.join("daemon.toml");
        std::fs::write(
            &path,
            format!(
                "[machine]\nkey_file = \"{}\"\n\n[daemon]\nport = 0\n\n[storage]\ndatabase = \"{}\"\n",
                key_file.display(),
                database.display()
            ),
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn pair_writes_only_the_token_file_and_leaves_config_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_config(dir.path());
        let before = std::fs::read(&config_path).unwrap();
        let url = start_gateway().await;

        pair(
            Some(config_path.clone()),
            url,
            CODE.into(),
            Some("name".into()),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&config_path).unwrap(), before);
        let token_path = dir.path().join("machine.key.token");
        let raw = std::fs::read_to_string(&token_path).unwrap();
        assert!(raw.trim().starts_with("flm_"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn pair_output_does_not_print_the_token() {
        let result = PairingResult {
            machine_id: "m1".into(),
            org_id: Some("org_default".into()),
            token: "flm_super_secret".into(),
        };
        let rendered = pair_summary(&result, Path::new("/tmp/machine.key.token")).to_string();
        assert!(!rendered.contains("flm_super_secret"));
        assert!(rendered.contains("token_file"));
    }

    #[tokio::test]
    async fn remove_workspace_requires_yes() {
        let error = remove_workspace(None, "ws_missing".into(), false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("--yes"), "{error}");
    }

    #[tokio::test]
    async fn token_file_permission_error_is_clear() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_config(dir.path());
        std::fs::create_dir(dir.path().join("machine.key.token")).unwrap();
        let url = start_gateway().await;

        let error = pair(Some(config_path), url, CODE.into(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("machine.key.token"),
            "the error should name the token path: {error}"
        );
    }
}

/// Rotate the machine key and leave a proof for the next connection.
///
/// The daemon must be stopped. The previous key is written to
/// `<key_file>.previous` and removed automatically after the agent admin
/// accepts the rotation during the next handshake.
pub async fn rotate_key(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let mut config = DaemonConfig::load(config_path.as_deref())?;
    config.resolve_paths();
    if !config.machine.key_file.exists() {
        anyhow::bail!(
            "machine key {} does not exist; start the daemon once first",
            config.machine.key_file.display()
        );
    }
    let previous = MachineIdentity::rotate_file(&config.machine.key_file)?;
    let current = MachineIdentity::load(&config.machine.key_file)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "key_file": config.machine.key_file,
            "previous_fingerprint": previous.fingerprint(),
            "new_fingerprint": current.fingerprint(),
            "note": "start the daemon and connect to the agent admin; the previous key is removed after the rotation handshake succeeds",
        }))?
    );
    Ok(())
}
