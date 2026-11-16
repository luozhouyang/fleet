//! Network detection and guided setup for Tailscale and Cloudflare Tunnel.
//!
//! The wizard never executes privileged networking commands. It detects the
//! local tooling, validates the admin endpoint, writes the admin/daemon/CLI
//! configuration and prints the exact commands the operator should run.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::AdminClient;
use crate::config::{
    backup_file, default_admin_config_path, default_cli_config_path, default_daemon_config_path,
    edit_toml, edit_toml_with_removals, home_dir, CliConfig, TomlEdit,
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

/// Resolve the CLI for a vendor tool. On macOS the Tailscale app does not
/// always put its CLI on PATH, so fall back to the app bundle.
pub(crate) fn tool_program(name: &str) -> String {
    if command_on_path(name) {
        return name.to_string();
    }
    #[cfg(target_os = "macos")]
    {
        if name == "tailscale" {
            let bundled = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";
            if std::path::Path::new(bundled).is_file() {
                return bundled.to_string();
            }
        }
    }
    name.to_string()
}

fn command_on_path(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkReport {
    pub admin: AdminProbe,
    pub tailscale: TailscaleReport,
    pub cloudflare: CloudflareReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminProbe {
    pub url: String,
    pub reachable: bool,
    pub version: Option<String>,
    pub machines_online: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TailscaleReport {
    pub installed: bool,
    pub version: Option<String>,
    pub backend_state: Option<String>,
    pub dns_name: Option<String>,
    pub ipv4: Vec<String>,
    pub running: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudflareReport {
    pub installed: bool,
    pub version: Option<String>,
    /// `None` when `cloudflared tunnel list` failed (usually not logged in).
    pub tunnels: Option<Vec<Tunnel>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    pub id: String,
    pub name: String,
}

impl NetworkReport {
    pub fn tailscale_ready(&self) -> bool {
        self.tailscale.running && self.tailscale.dns_name.is_some()
    }

    pub fn cloudflare_ready(&self) -> bool {
        self.cloudflare.installed
    }
}

pub async fn detect(admin_url: &str, token: Option<String>) -> NetworkReport {
    let admin = probe_admin(admin_url, token).await;
    let tailscale = detect_tailscale().await;
    let cloudflare = detect_cloudflare().await;
    NetworkReport {
        admin,
        tailscale,
        cloudflare,
    }
}

async fn probe_admin(url: &str, token: Option<String>) -> AdminProbe {
    let client = match AdminClient::new(url, token) {
        Ok(client) => client,
        Err(error) => {
            return AdminProbe {
                url: url.to_string(),
                reachable: false,
                version: None,
                machines_online: None,
                error: Some(error.to_string()),
            };
        }
    };
    match client.health().await {
        Ok(value) => AdminProbe {
            url: url.to_string(),
            reachable: true,
            version: value
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            machines_online: value.get("machines_online").and_then(Value::as_u64),
            error: None,
        },
        Err(error) => AdminProbe {
            url: url.to_string(),
            reachable: false,
            version: None,
            machines_online: None,
            error: Some(error.to_string()),
        },
    }
}

async fn detect_tailscale() -> TailscaleReport {
    let mut report = TailscaleReport {
        installed: false,
        version: None,
        backend_state: None,
        dns_name: None,
        ipv4: Vec::new(),
        running: false,
        error: None,
    };
    let program = tool_program("tailscale");
    let version = match run_command(&program, &["version"]).await {
        Ok(output) => output,
        Err(error) => {
            report.error = Some(format!("tailscale not available: {error}"));
            return report;
        }
    };
    report.installed = true;
    report.version = version.lines().next().map(str::trim).map(str::to_string);

    match run_command(&program, &["status", "--json"]).await {
        Ok(raw) => match parse_tailscale_status(&raw) {
            Ok(status) => {
                report.backend_state = status.backend_state.clone();
                report.running = status.backend_state.as_deref() == Some("Running");
                report.dns_name = status.dns_name;
                report.ipv4 = status.ipv4;
            }
            Err(error) => report.error = Some(format!("invalid tailscale status JSON: {error}")),
        },
        Err(error) => report.error = Some(format!("tailscale status failed: {error}")),
    }
    report
}

async fn detect_cloudflare() -> CloudflareReport {
    let mut report = CloudflareReport {
        installed: false,
        version: None,
        tunnels: None,
        error: None,
    };
    let program = tool_program("cloudflared");
    let version = match run_command(&program, &["--version"]).await {
        Ok(output) => output,
        Err(error) => {
            report.error = Some(format!("cloudflared not available: {error}"));
            return report;
        }
    };
    report.installed = true;
    report.version = version.lines().next().map(str::trim).map(str::to_string);

    match run_command(&program, &["tunnel", "list", "--output", "json"]).await {
        Ok(raw) => match parse_cloudflared_tunnels(&raw) {
            Ok(tunnels) => report.tunnels = Some(tunnels),
            Err(error) => report.error = Some(format!("invalid tunnel list JSON: {error}")),
        },
        Err(error) => {
            report.error = Some(format!(
                "cloudflared tunnel list failed (are you logged in?): {error}"
            ));
        }
    }
    report
}

#[derive(Debug, Deserialize)]
struct TailscaleStatus {
    #[serde(rename = "BackendState")]
    backend_state: Option<String>,
    #[serde(rename = "Self")]
    this_node: Option<TailscaleSelf>,
}

#[derive(Debug, Deserialize)]
struct TailscaleSelf {
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
    #[serde(rename = "TailscaleIPs", default)]
    ips: Vec<String>,
}

#[derive(Debug, Default)]
struct ParsedTailscale {
    backend_state: Option<String>,
    dns_name: Option<String>,
    ipv4: Vec<String>,
}

fn parse_tailscale_status(raw: &str) -> Result<ParsedTailscale> {
    let status: TailscaleStatus = serde_json::from_str(raw)?;
    let mut parsed = ParsedTailscale {
        backend_state: status.backend_state,
        ..Default::default()
    };
    if let Some(node) = status.this_node {
        parsed.dns_name = node
            .dns_name
            .map(|name| name.trim_end_matches('.').to_string())
            .filter(|name| !name.is_empty());
        parsed.ipv4 = node
            .ips
            .into_iter()
            .filter(|ip| ip.contains('.') && !ip.contains(':'))
            .collect();
    }
    Ok(parsed)
}

fn parse_cloudflared_tunnels(raw: &str) -> Result<Vec<Tunnel>> {
    let value: Value = serde_json::from_str(raw)?;
    let Some(items) = value.as_array() else {
        bail!("expected a JSON array of tunnels");
    };
    let mut tunnels = Vec::new();
    for item in items {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
        if !id.is_empty() && !name.is_empty() {
            tunnels.push(Tunnel {
                id: id.to_string(),
                name: name.to_string(),
            });
        }
    }
    Ok(tunnels)
}

async fn run_command(program: &str, args: &[&str]) -> Result<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .with_context(|| format!("`{program}` timed out"))?
        .with_context(|| format!("failed to execute `{program}`"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "`{program} {}` exited with {}: {stderr}",
            args.join(" "),
            output.status
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// Wizard
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    Tailscale,
    Cloudflare,
}

#[derive(Debug, Clone)]
pub struct WizardOptions {
    pub mode: Option<NetworkMode>,
    pub hostname: Option<String>,
    pub admin_url: String,
    pub token: Option<String>,
    pub admin_config: PathBuf,
    pub daemon_config: PathBuf,
    pub cli_config: PathBuf,
    pub cloudflared_config: PathBuf,
    pub yes: bool,
}

impl WizardOptions {
    pub fn new(admin_url: String, token: Option<String>) -> Self {
        Self {
            mode: None,
            hostname: None,
            admin_url,
            token,
            admin_config: default_admin_config_path(),
            daemon_config: default_daemon_config_path(),
            cli_config: default_cli_config_path(),
            cloudflared_config: home_dir().join(".fleet").join("cloudflared.yml"),
            yes: false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) mode: NetworkMode,
    pub(crate) hostname: String,
    pub(crate) public_url: String,
    pub(crate) ws_url: String,
    pub(crate) local_url: String,
    pub(crate) commands: Vec<String>,
    pub(crate) tunnel: Option<Tunnel>,
}

pub async fn wizard(options: WizardOptions) -> Result<()> {
    println!("Detecting local network tooling…");
    let report = detect(&options.admin_url, options.token.clone()).await;
    print_report(&report);

    if !report.admin.reachable {
        println!(
            "\nwarning: {} is not reachable; configuration will still be generated.",
            report.admin.url
        );
    }

    let mode = match options.mode {
        Some(mode) => mode,
        None => choose_mode(&report, options.yes)?,
    };
    let hostname = resolve_hostname(&report, mode, options.hostname.clone(), options.yes)?;
    // Only reuse our own tunnel name; never hijack an unrelated tunnel.
    let tunnel = report.cloudflare.tunnels.as_ref().and_then(|tunnels| {
        tunnels
            .iter()
            .find(|tunnel| tunnel.name == "fleet")
            .cloned()
    });
    let plan = build_plan(mode, hostname, tunnel, &options.admin_url);

    print_plan(&plan);
    if !options.yes && !confirm("Apply these configuration changes?")? {
        println!("aborted; nothing was written.");
        return Ok(());
    }

    let tunnel_written = apply_plan(&plan, &options)?;
    print_next_steps(&plan, &options, tunnel_written);
    Ok(())
}

fn choose_mode(report: &NetworkReport, yes: bool) -> Result<NetworkMode> {
    if report.tailscale_ready() {
        return Ok(NetworkMode::Tailscale);
    }
    if report.cloudflare_ready() {
        if report.cloudflare.tunnels.is_some() {
            return Ok(NetworkMode::Cloudflare);
        }
        if yes {
            return Ok(NetworkMode::Cloudflare);
        }
        println!(
            "\nwarning: cloudflared is installed but not authenticated; run `cloudflared tunnel login` first."
        );
    }
    if yes {
        bail!(
            "no ready network tool found; install/start Tailscale or install cloudflared, or pass --mode"
        );
    }
    println!("\nChoose a remote access mode:");
    println!("  1) Tailscale Serve (recommended: private tailnet, automatic TLS)");
    println!("  2) Cloudflare Tunnel (public hostname or Cloudflare Access)");
    loop {
        let answer = prompt("Mode [1/2]: ", None)?;
        match answer.trim() {
            "1" => return Ok(NetworkMode::Tailscale),
            "2" => return Ok(NetworkMode::Cloudflare),
            _ => println!("Enter 1 or 2."),
        }
    }
}

pub(crate) fn resolve_hostname(
    report: &NetworkReport,
    mode: NetworkMode,
    provided: Option<String>,
    yes: bool,
) -> Result<String> {
    if let Some(hostname) = provided {
        return sanitize_hostname(&hostname);
    }
    match mode {
        NetworkMode::Tailscale => {
            if let Some(dns_name) = &report.tailscale.dns_name {
                return sanitize_hostname(dns_name);
            }
            bail!(
                "Tailscale is not running or has no DNS name; run `tailscale up` or pass --hostname"
            );
        }
        NetworkMode::Cloudflare => {
            if yes {
                bail!("Cloudflare mode requires --hostname in non-interactive mode");
            }
            let answer = prompt(
                "Public hostname for the admin (e.g. fleet.example.com): ",
                None,
            )?;
            sanitize_hostname(&answer)
        }
    }
}

pub(crate) fn sanitize_hostname(input: &str) -> Result<String> {
    let hostname = input.trim().trim_end_matches('/');
    if hostname.is_empty() {
        bail!("hostname must not be empty");
    }
    if hostname.contains("://") || hostname.contains('/') {
        bail!("hostname must not include a scheme or path (got `{input}`)");
    }
    if !hostname
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        bail!("hostname contains unsupported characters: `{input}`");
    }
    Ok(hostname.to_string())
}

fn local_port(admin_url: &str) -> String {
    url::port_or_default(admin_url)
}

pub(crate) fn build_plan(
    mode: NetworkMode,
    hostname: String,
    tunnel: Option<Tunnel>,
    admin_url: &str,
) -> Plan {
    let port = local_port(admin_url);
    let local_url = format!("http://127.0.0.1:{port}");
    let public_url = format!("https://{hostname}");
    let ws_url = format!("wss://{hostname}/ws/machine");
    let commands = match mode {
        NetworkMode::Tailscale => vec![
            "tailscale up".to_string(),
            format!("sudo tailscale serve --bg --https=443 {local_url}"),
            format!("curl -fsS {public_url}/api/v1/health"),
            format!("# daemon: FLEET_SERVER_URL={ws_url}"),
        ],
        NetworkMode::Cloudflare => {
            let tunnel_name = tunnel
                .as_ref()
                .map(|tunnel| tunnel.name.clone())
                .unwrap_or_else(|| "fleet".to_string());
            vec![
                "cloudflared tunnel login".to_string(),
                format!("cloudflared tunnel create {tunnel_name}"),
                format!("cloudflared tunnel route dns {tunnel_name} {hostname}"),
                format!("cloudflared tunnel --config ~/.fleet/cloudflared.yml run {tunnel_name}"),
                format!("curl -fsS {public_url}/api/v1/health"),
                "# recommended: put Cloudflare Access in front of the web UI".to_string(),
                format!("# daemon: FLEET_SERVER_URL={ws_url}"),
            ]
        }
    };
    Plan {
        mode,
        hostname,
        public_url,
        ws_url,
        local_url,
        commands,
        tunnel,
    }
}

pub(crate) fn apply_plan(plan: &Plan, options: &WizardOptions) -> Result<bool> {
    // CLI settings: record the public URL and keep the local admin endpoint.
    let mut cli = CliConfig::load(&options.cli_config)?;
    if cli.admin_url.is_none() {
        cli.admin_url = Some(options.admin_url.clone());
    }
    cli.public_url = Some(plan.public_url.clone());
    cli.save(&options.cli_config)?;
    println!("updated {}", options.cli_config.display());

    // Admin: advertise the public URL (bind stays loopback by default).
    let backup = edit_toml(
        &options.admin_config,
        &[TomlEdit {
            section: "server",
            key: "public_url",
            value: plan.public_url.clone(),
        }],
    )?;
    println!(
        "updated {}{}",
        options.admin_config.display(),
        backup
            .map(|path| format!(" (backup: {})", path.display()))
            .unwrap_or_default()
    );

    // Daemon: point the reverse connection at the public WebSocket endpoint.
    // Point the daemon at the public endpoint and drop any legacy shared
    // token the wizard's earlier versions may have written.
    let backup = edit_toml_with_removals(
        &options.daemon_config,
        &[TomlEdit {
            section: "connection",
            key: "server_url",
            value: plan.ws_url.clone(),
        }],
        &[("connection", "token")],
    )?;
    println!(
        "updated {}{}",
        options.daemon_config.display(),
        backup
            .map(|path| format!(" (backup: {})", path.display()))
            .unwrap_or_default()
    );

    let mut tunnel_written = false;
    if plan.mode == NetworkMode::Cloudflare {
        if plan.tunnel.is_some() {
            write_cloudflared_config(plan, &options.cloudflared_config)?;
            tunnel_written = true;
        } else {
            println!(
                "skipped {}: tunnel `fleet` not found yet; create it and re-run the wizard",
                options.cloudflared_config.display()
            );
        }
    }
    Ok(tunnel_written)
}

fn write_cloudflared_config(plan: &Plan, path: &Path) -> Result<()> {
    let backup = if path.exists() {
        Some(backup_file(path)?)
    } else {
        None
    };
    let tunnel_name = plan
        .tunnel
        .as_ref()
        .map(|tunnel| tunnel.name.as_str())
        .unwrap_or("fleet");
    let tunnel_id = plan
        .tunnel
        .as_ref()
        .map(|tunnel| tunnel.id.as_str())
        .unwrap_or("<tunnel-id>");
    let content = format!(
        "# Generated by `fleet network wizard`.\n\
         tunnel: {tunnel_name}\n\
         credentials-file: {}/.cloudflared/{tunnel_id}.json\n\
         ingress:\n\
         \x20 - hostname: {hostname}\n\
         \x20   service: {local_url}\n\
         \x20 - service: http_status:404\n",
        home_dir().display(),
        hostname = plan.hostname,
        local_url = plan.local_url
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
    println!(
        "updated {}{}",
        path.display(),
        backup
            .map(|path| format!(" (backup: {})", path.display()))
            .unwrap_or_default()
    );
    Ok(())
}

pub fn print_report(report: &NetworkReport) {
    println!("\nAdmin endpoint");
    println!(
        "  {}  {}",
        report.admin.url,
        if report.admin.reachable {
            "reachable"
        } else {
            "unreachable"
        }
    );
    if let Some(version) = &report.admin.version {
        println!("  version {version}");
    }
    if let Some(error) = &report.admin.error {
        println!("  {error}");
    }

    println!("\nTailscale");
    if report.tailscale.installed {
        println!(
            "  installed  {}{}",
            report.tailscale.version.as_deref().unwrap_or(""),
            if report.tailscale.running {
                " (running)"
            } else {
                " (not running)"
            }
        );
        if let Some(dns) = &report.tailscale.dns_name {
            println!("  dns        {dns}");
        }
        if !report.tailscale.ipv4.is_empty() {
            println!("  ipv4       {}", report.tailscale.ipv4.join(", "));
        }
        if let Some(error) = &report.tailscale.error {
            println!("  {error}");
        }
    } else {
        println!("  not installed");
    }

    println!("\nCloudflare Tunnel");
    if report.cloudflare.installed {
        println!(
            "  installed  {}",
            report.cloudflare.version.as_deref().unwrap_or("")
        );
        match &report.cloudflare.tunnels {
            Some(tunnels) if tunnels.is_empty() => println!("  tunnels   (none)"),
            Some(tunnels) => {
                println!("  tunnels");
                for tunnel in tunnels {
                    println!("    {}  {}", tunnel.name, tunnel.id);
                }
            }
            None => println!("  tunnels   unavailable (run `cloudflared tunnel login`)"),
        }
        if let Some(error) = &report.cloudflare.error {
            println!("  {error}");
        }
    } else {
        println!("  not installed");
    }
}

pub(crate) fn print_plan(plan: &Plan) {
    println!(
        "\nPlan: {} via {}",
        match plan.mode {
            NetworkMode::Tailscale => "Tailscale Serve",
            NetworkMode::Cloudflare => "Cloudflare Tunnel",
        },
        plan.hostname
    );
    println!("  public URL    {}", plan.public_url);
    println!("  daemon WS URL {}", plan.ws_url);
    println!("\nCommands to run yourself:");
    for command in &plan.commands {
        println!("  {command}");
    }
}

fn print_next_steps(plan: &Plan, options: &WizardOptions, tunnel_written: bool) {
    println!("\nConfiguration written:");
    println!("  CLI      {}", options.cli_config.display());
    println!("  admin    {}", options.admin_config.display());
    println!("  daemon   {}", options.daemon_config.display());
    if tunnel_written {
        println!("  tunnel   {}", options.cloudflared_config.display());
    }
    println!(
        "\nRestart the admin and daemon, then verify {} and reconnect the daemon.",
        plan.public_url
    );
}

fn prompt(message: &str, default: Option<&str>) -> Result<String> {
    print!("{message}");
    if let Some(default) = default {
        print!("[{default}] ");
    }
    io::stdout().flush().context("failed to flush stdout")?;
    let mut line = String::new();
    let read = io::stdin()
        .lock()
        .read_line(&mut line)
        .context("failed to read stdin")?;
    if read == 0 {
        bail!("stdin closed; pass the corresponding flag for non-interactive use");
    }
    let line = line.trim();
    if line.is_empty() {
        Ok(default.unwrap_or_default().to_string())
    } else {
        Ok(line.to_string())
    }
}

fn confirm(message: &str) -> Result<bool> {
    let answer = prompt(&format!("{message} [y/N]: "), Some("n"))?;
    Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

mod url {
    pub fn port_or_default(base: &str) -> String {
        let without_scheme = base.split_once("://").map(|(_, rest)| rest).unwrap_or(base);
        let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
        match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(']') || host.ends_with(']') => {
                if port.chars().all(|ch| ch.is_ascii_digit()) && !port.is_empty() {
                    port.to_string()
                } else {
                    default_port(base)
                }
            }
            _ => default_port(base),
        }
    }

    fn default_port(base: &str) -> String {
        if base.starts_with("https://") {
            "443".to_string()
        } else {
            "80".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tailscale_status() {
        let raw = r#"{
            "BackendState": "Running",
            "Self": {
                "DNSName": "fleet-host.tailnet-1234.ts.net.",
                "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"]
            }
        }"#;
        let parsed = parse_tailscale_status(raw).unwrap();
        assert_eq!(parsed.backend_state.as_deref(), Some("Running"));
        assert_eq!(
            parsed.dns_name.as_deref(),
            Some("fleet-host.tailnet-1234.ts.net")
        );
        assert_eq!(parsed.ipv4, vec!["100.101.102.103"]);
    }

    #[test]
    fn parses_cloudflared_tunnels() {
        let raw = r#"[{"id":"abc-123","name":"fleet"},{"id":"def","name":"other"}]"#;
        let tunnels = parse_cloudflared_tunnels(raw).unwrap();
        assert_eq!(tunnels.len(), 2);
        assert_eq!(tunnels[0].name, "fleet");
    }

    #[test]
    fn builds_urls_and_port() {
        let plan = build_plan(
            NetworkMode::Tailscale,
            "host.tailnet.ts.net".into(),
            None,
            "http://127.0.0.1:8787",
        );
        assert_eq!(plan.public_url, "https://host.tailnet.ts.net");
        assert_eq!(plan.ws_url, "wss://host.tailnet.ts.net/ws/machine");
        assert!(plan
            .commands
            .iter()
            .any(|command| command.contains("127.0.0.1:8787")));
        assert_eq!(url::port_or_default("https://example.com"), "443");
    }

    #[test]
    fn writes_configuration_and_backups() {
        let dir = tempfile::tempdir().unwrap();
        let admin = dir.path().join("config.toml");
        let daemon = dir.path().join("config.toml");
        fs::write(
            &admin,
            "# existing\n[server]\nhost = \"127.0.0.1\"\nport = 8787\n",
        )
        .unwrap();
        let plan = build_plan(
            NetworkMode::Tailscale,
            "host.tailnet.ts.net".into(),
            None,
            "http://127.0.0.1:8787",
        );
        let options = WizardOptions {
            admin_url: "http://127.0.0.1:8787".into(),
            admin_config: admin.clone(),
            daemon_config: daemon.clone(),
            cli_config: dir.path().join("cli.toml"),
            cloudflared_config: dir.path().join("cloudflared.yml"),
            ..WizardOptions::new("http://127.0.0.1:8787".into(), None)
        };
        apply_plan(&plan, &options).unwrap();

        let raw = fs::read_to_string(&admin).unwrap();
        assert!(raw.contains("# existing"));
        assert!(raw.contains("public_url = \"https://host.tailnet.ts.net\""));
        let parsed: toml::Value = toml::from_str(&raw).unwrap();
        assert_eq!(parsed["server"]["port"].as_integer(), Some(8787));
        assert!(dir.path().join("config.toml.bak").exists());

        let parsed: toml::Value = toml::from_str(&fs::read_to_string(&daemon).unwrap()).unwrap();
        assert_eq!(
            parsed["connection"]["server_url"].as_str(),
            Some("wss://host.tailnet.ts.net/ws/machine")
        );
    }

    #[test]
    fn sanitizes_hostnames() {
        assert_eq!(
            sanitize_hostname("fleet.example.com/").unwrap(),
            "fleet.example.com"
        );
        assert!(sanitize_hostname("https://fleet.example.com").is_err());
        assert!(sanitize_hostname("bad host").is_err());
    }
}
