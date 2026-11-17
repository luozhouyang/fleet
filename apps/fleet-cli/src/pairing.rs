//! `fleet daemon pair` and `fleet admin machines pair-code`.
//!
//! Pairing loads the machine identity, redeems a one-shot pairing code, and
//! stores the returned per-machine token next to the key as `<key_file>.token`.
//! The daemon configuration is not rewritten: `fleet daemon setup` owns that.

use std::path::Path;

use anyhow::{bail, Result};
use fleet_core_auth::{
    load_or_create_machine_id, machine_token_path, normalize_pair_code, save_machine_token,
    MachineIdentity,
};
use fleet_core_config::DaemonConfig;
use fleet_core_connection::{pair_machine, PairingConfig};
use fleet_core_protocol::pairing::{CreatePairingCodeRequest, PairingCodeResponse};
use fleet_core_protocol::PROTOCOL_VERSION;

use crate::client::AdminClient;
use crate::config::expand_tilde;
use crate::output;

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

/// Redeem `code` and store the per-machine token next to `key_file`.
///
/// Returns the machine id the admin paired. The caller owns URL validation and
/// configuration writes; this function only touches identity files and the
/// token file.
pub async fn pair_with_code(
    server: &str,
    code: &str,
    machine_id: &str,
    machine_name: &str,
    key_file: &Path,
) -> Result<String> {
    // Normalize before touching the filesystem so an invalid code cannot
    // create a machine key.
    let code = normalize_pair_code(code).ok_or_else(|| {
        anyhow::anyhow!("invalid pairing code; expected 12 Crockford Base32 symbols")
    })?;

    let identity = MachineIdentity::load_or_create(key_file)?;
    let previous = MachineIdentity::load_previous(key_file)?;
    let result = pair_machine(
        PairingConfig {
            server_url: server.to_string(),
            machine_id: machine_id.to_string(),
            machine_name: machine_name.to_string(),
            hostname: fleet_core_connection::local_hostname(),
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

    save_machine_token(&machine_token_path(key_file), &result.token)?;
    Ok(result.machine_id)
}

/// Pair using the configuration at `config_path` without rewriting it.
///
/// Only `<key_file>.token` (and, for a fresh machine, the key and id files)
/// are written.
pub async fn pair_only(
    config_path: &Path,
    server: String,
    code: String,
    name: Option<String>,
) -> Result<()> {
    let config = DaemonConfig::load(Some(config_path))?;
    let key_file = expand_tilde(config.machine.key_file.clone());
    let identity = MachineIdentity::load_or_create(&key_file)?;
    let machine_id = match config.machine.id.clone() {
        Some(id) => id,
        None => load_or_create_machine_id(&key_file, &identity)?,
    };
    let machine_name = name
        .or_else(|| config.machine.name.clone())
        .unwrap_or_else(fleet_core_connection::default_machine_name);

    let paired = pair_with_code(&server, &code, &machine_id, &machine_name, &key_file).await?;
    println!("paired machine {paired}");
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
    async fn invalid_code_does_not_create_identity_files() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("machine.key");
        let error = pair_with_code(
            "ws://127.0.0.1:9/ws/machine",
            "short",
            "machine_1",
            "machine",
            &key_file,
        )
        .await
        .expect_err("invalid code");
        assert!(error.to_string().contains("invalid pairing code"));
        assert!(!key_file.exists());
        assert!(!machine_token_path(&key_file).exists());
    }
}
