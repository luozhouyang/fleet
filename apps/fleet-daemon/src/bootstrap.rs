use std::path::PathBuf;

use fleet_core_auth::MachineIdentity;
use fleet_core_config::DaemonConfig;
use fleet_core_protocol::workspace::RegisterWorkspaceRequest;

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
