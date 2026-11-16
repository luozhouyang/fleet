//! `fleet daemon setup` and `fleet admin machines pair-code`.
//!
//! `fleet daemon setup` walks a fresh machine through pairing: it probes the admin,
//! loads or creates the machine identity, asks the admin for a one-shot pairing
//! code, completes the pairing handshake and stores the returned per-machine
//! token next to the key. It then rewrites the daemon TOML to point at the
//! server and removes the legacy `connection.token` / `connection.pairing_code`
//! fields, because tokens no longer live in configuration.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use fleet_core_auth::{
    load_or_create_machine_id, machine_token_path, save_machine_token, MachineIdentity,
};
use fleet_core_config::DaemonConfig;
use fleet_core_connection::{pair_machine, PairingConfig};
use fleet_core_protocol::pairing::{CreatePairingCodeRequest, PairingCodeResponse};
use fleet_core_protocol::PROTOCOL_VERSION;

use crate::client::AdminClient;
use crate::config::{edit_toml_with_removals, CliConfig, TomlEdit};
use crate::output;

pub struct SetupOptions {
    pub admin_url: String,
    pub token: Option<String>,
    pub org_id: Option<String>,
    pub server_url: Option<String>,
    pub daemon_config: PathBuf,
    pub name: Option<String>,
    pub store_token: bool,
    pub allow_insecure: bool,
    pub configure_only: bool,
    /// CLI configuration written by `--store-token` (the active `--config`).
    pub cli_config: Option<PathBuf>,
}

/// Map the admin's HTTP base URL to the daemon's machine WebSocket endpoint.
pub fn derive_ws_url(admin_url: &str) -> Result<String> {
    let admin_url = admin_url.trim().trim_end_matches('/');
    let ws = if let Some(rest) = admin_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = admin_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        bail!("admin URL must start with http:// or https://");
    };
    Ok(format!("{ws}/ws/machine"))
}

/// Whether a host is safe for plaintext `ws://` in local development.
pub fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split('/').next().unwrap_or_default();
    // Drop optional userinfo (`user:pass@host`).
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    // Bracketed IPv6, e.g. `[::1]:8787`.
    if authority.starts_with('[') {
        if let Some(end) = authority.find(']') {
            return Some(&authority[..=end]);
        }
    }
    Some(authority.split(':').next().unwrap_or_default())
}

/// Validate the daemon WebSocket URL before pairing.
///
/// Only `ws://` and `wss://` are accepted. Plaintext `ws://` to a non-loopback
/// host requires `--allow-insecure`; this also covers `--server http://...`,
/// which used to bypass the check.
pub fn validate_server_url(server_url: &str, allow_insecure: bool) -> Result<()> {
    let (scheme, _) = server_url
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("--server must be a ws:// or wss:// URL"))?;
    match scheme {
        "ws" => {
            if let Some(host) = host_of(server_url) {
                if !is_loopback(host) && !allow_insecure {
                    bail!(
                        "refusing plaintext ws:// to {host}; pass --allow-insecure on a trusted LAN"
                    );
                }
            }
            Ok(())
        }
        "wss" => Ok(()),
        other => bail!("--server must be a ws:// or wss:// URL, not `{other}://`"),
    }
}

pub async fn setup(options: SetupOptions) -> Result<()> {
    let server_url = match options.server_url.clone() {
        Some(url) => url,
        None => derive_ws_url(&options.admin_url)?,
    };
    validate_server_url(&server_url, options.allow_insecure)?;

    // `--configure-only` performs no network calls: it is for a machine that
    // already has a token.
    if !options.configure_only {
        let client = AdminClient::new(&options.admin_url, options.token.clone())?;
        client
            .health()
            .await
            .context("fleet admin is not reachable")?;
    }

    let mut config = DaemonConfig::load(Some(&options.daemon_config))?;
    config.resolve_paths();
    let key_file = config.machine.key_file.clone();
    let identity = MachineIdentity::load_or_create(&key_file)?;
    let previous = MachineIdentity::load_previous(&key_file)?;
    let machine_id = match config.machine.id.clone() {
        Some(id) => id,
        None => load_or_create_machine_id(&key_file, &identity)?,
    };

    if !options.configure_only {
        let token = options.token.clone().context(
            "an admin token is required to create a pairing code; pass --admin-token or set FLEET_ADMIN_TOKEN",
        )?;
        let client = AdminClient::new(&options.admin_url, Some(token))?;
        let request = CreatePairingCodeRequest {
            org_id: options.org_id.clone(),
            machine_name: options.name.clone(),
        };
        let value = client
            .post("/api/v1/pairing/codes", serde_json::to_value(request)?)
            .await?;
        let code: PairingCodeResponse = serde_json::from_value(value)?;

        let result = pair_machine(
            PairingConfig {
                server_url: server_url.clone(),
                machine_id: machine_id.clone(),
                machine_name: options.name.clone().unwrap_or_else(|| machine_id.clone()),
                hostname: fleet_core_connection::local_hostname(),
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                protocol_version: PROTOCOL_VERSION,
                pairing_code: code.code,
                handshake_timeout: std::time::Duration::from_secs(10),
            },
            &identity,
            previous.as_ref(),
        )
        .await?;

        save_machine_token(&machine_token_path(&key_file), &result.token)?;
        println!(
            "paired machine {} into {}",
            result.machine_id,
            result.org_id.as_deref().unwrap_or("unknown")
        );
    }

    let mut edits = vec![TomlEdit {
        section: "connection",
        key: "server_url",
        value: server_url.clone(),
    }];
    edits.push(TomlEdit {
        section: "machine",
        key: "id",
        value: machine_id.clone(),
    });
    if let Some(name) = options.name.as_deref() {
        edits.push(TomlEdit {
            section: "machine",
            key: "name",
            value: name.to_string(),
        });
    }
    // One backed-up pass for both the edits and the legacy-key removal.
    let backup = edit_toml_with_removals(
        &options.daemon_config,
        &edits,
        &[("connection", "token"), ("connection", "pairing_code")],
    )?;

    if options.store_token {
        let path = options
            .cli_config
            .clone()
            .unwrap_or_else(crate::config::default_cli_config_path);
        let mut cli = CliConfig::load(&path)?;
        // Only replace a token when one was supplied: `--configure-only
        // --store-token` must not erase a previously stored token.
        if let Some(token) = options.token.clone() {
            cli.token = Some(token);
        }
        cli.admin_url = Some(options.admin_url.clone());
        cli.save(&path)?;
    }

    println!(
        "updated {}{}",
        options.daemon_config.display(),
        backup
            .map(|path| format!(" (backup: {})", path.display()))
            .unwrap_or_default()
    );
    println!("next: start the daemon so it connects with its per-machine token");
    Ok(())
}

pub async fn create_code(
    client: &AdminClient,
    org_id: Option<String>,
    machine_name: Option<String>,
    json: bool,
) -> Result<()> {
    let value = client
        .post(
            "/api/v1/pairing/codes",
            serde_json::to_value(CreatePairingCodeRequest {
                org_id,
                machine_name,
            })?,
        )
        .await?;
    let response: PairingCodeResponse = serde_json::from_value(value)?;
    if json {
        return output::print_json(&serde_json::to_value(&response)?);
    }
    print!("{}", pair_code_text(&response));
    Ok(())
}

/// Human-readable `fleet admin machines pair-code` output.
fn pair_code_text(response: &PairingCodeResponse) -> String {
    format!(
        "code      {}\nexpires   {}\norg       {}\n\nthis code is shown once; do not paste it into shell history\n",
        response.code, response.expires_at, response.org_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_ws_url_from_admin_url() {
        assert_eq!(
            derive_ws_url("http://127.0.0.1:8787").unwrap(),
            "ws://127.0.0.1:8787/ws/machine"
        );
        assert_eq!(
            derive_ws_url("https://fleet.example.com").unwrap(),
            "wss://fleet.example.com/ws/machine"
        );
        assert!(derive_ws_url("ws://elsewhere").is_err());
    }

    #[test]
    fn refuses_insecure_non_loopback() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("localhost"));
        assert!(is_loopback("::1"));
        assert!(!is_loopback("192.168.1.5"));
    }

    #[test]
    fn the_setup_guard_refuses_plaintext_to_non_loopback() {
        assert!(validate_server_url("ws://192.168.1.5:8787/ws/machine", false).is_err());
        assert!(validate_server_url("ws://192.168.1.5:8787/ws/machine", true).is_ok());
        assert!(validate_server_url("wss://fleet.example.com/ws/machine", false).is_ok());
        assert!(validate_server_url("ws://127.0.0.1:8787/ws/machine", false).is_ok());
    }

    #[test]
    fn the_setup_guard_rejects_http_and_unknown_schemes() {
        assert!(validate_server_url("http://192.168.1.5:8787/ws/machine", true).is_err());
        assert!(validate_server_url("WS://127.0.0.1:8787/ws/machine", true).is_err());
        assert!(validate_server_url("127.0.0.1:8787", true).is_err());
    }

    #[test]
    fn extracts_hosts_including_ipv6_and_userinfo() {
        assert_eq!(host_of("ws://127.0.0.1:8787/ws/machine"), Some("127.0.0.1"));
        assert_eq!(
            host_of("wss://fleet.example.com/ws/machine"),
            None,
            "only ws:// and http:// are considered"
        );
        assert_eq!(host_of("ws://[::1]:8787/ws/machine"), Some("[::1]"));
        assert_eq!(
            host_of("ws://operator:secret@192.168.1.5:8787/ws/machine"),
            Some("192.168.1.5")
        );
    }

    #[test]
    fn pair_code_output_includes_the_code_and_expiry() {
        let response = fleet_core_protocol::pairing::PairingCodeResponse {
            id: "pair_1".into(),
            code: "ABCD-EFGH-JKMN".into(),
            expires_at: chrono::Utc::now(),
            org_id: "org_default".into(),
        };
        let text = pair_code_text(&response);
        assert!(text.contains("ABCD-EFGH-JKMN"));
        assert!(text.contains(&response.expires_at.to_string()));
        assert!(text.contains("org_default"));
    }

    #[tokio::test]
    async fn configure_only_makes_no_network_call() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("daemon.toml");
        std::fs::write(
            &config_path,
            format!(
                "[machine]\nkey_file = \"{}\"\n\n[storage]\ndatabase = \"{}\"\n",
                dir.path().join("machine.key").display(),
                dir.path().join("fleet-daemon.db").display()
            ),
        )
        .unwrap();

        // Port 9 is not listening: a health probe would fail, so this proves
        // the configure-only path performs no network call.
        setup(SetupOptions {
            admin_url: "http://127.0.0.1:9".into(),
            token: None,
            org_id: None,
            server_url: None,
            daemon_config: config_path.clone(),
            name: None,
            store_token: false,
            allow_insecure: false,
            configure_only: true,
            cli_config: None,
        })
        .await
        .unwrap();

        let raw = std::fs::read_to_string(&config_path).unwrap();
        assert!(raw.contains("server_url"));
        assert!(raw.contains("[machine]"));
        assert!(!dir.path().join("machine.key.token").exists());
    }
}
