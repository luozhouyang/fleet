//! Authorized, guided remote-access setup.
//!
//! Authorization uses the vendors' official browser flows:
//! - Tailscale: `tailscale up` device authorization.
//! - Cloudflare: `cloudflared tunnel login`.
//!
//! All external/privileged commands are opt-in (`--execute`) and confirmed
//! individually unless `--yes` is passed. Configuration writes are backed up.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::process::Command;

use crate::cloudflare::{AccessAllow, CloudflareApi};
use crate::network::{
    self, build_plan, sanitize_hostname, NetworkMode, Plan, Tunnel, WizardOptions,
};
use crate::secrets;

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub provider: NetworkMode,
    pub hostname: Option<String>,
    pub admin_url: String,
    pub admin_config: PathBuf,
    pub daemon_config: PathBuf,
    pub cli_config: PathBuf,
    pub cloudflared_config: PathBuf,
    pub execute: bool,
    pub yes: bool,
    pub cloudflare_api_token: Option<String>,
    pub cloudflare_account_id: Option<String>,
    pub access_email: Option<String>,
    pub access_domain: Option<String>,
    pub access_everyone: bool,
    pub skip_access: bool,
    pub skip_service: bool,
    /// Offer to install the missing vendor tool with Homebrew (macOS/Linuxbrew).
    pub install: bool,
}

pub async fn connect(options: ConnectOptions) -> Result<()> {
    println!("Detecting local network tooling…");
    let report = network::detect(&options.admin_url, None).await;
    network::print_report(&report);

    match options.provider {
        NetworkMode::Tailscale => connect_tailscale(&options, report).await,
        NetworkMode::Cloudflare => connect_cloudflare(&options, report).await,
    }
}

async fn connect_tailscale(
    options: &ConnectOptions,
    mut report: network::NetworkReport,
) -> Result<()> {
    let runner = Runner::new(options.execute, options.yes);
    let program = network::tool_program("tailscale");
    if !report.tailscale.installed {
        if !ensure_tool(options, &runner, Tool::Tailscale).await? {
            println!("\nPreview only. Re-run with --execute to install and continue.");
            return Ok(());
        }
        report = network::detect(&options.admin_url, None).await;
        if !report.tailscale.installed {
            bail!(
                "Tailscale is still not available; install it from https://tailscale.com/download and re-run"
            );
        }
    }
    if !report.tailscale_ready() {
        runner
            .command(
                "Authorize this machine with Tailscale (browser login)",
                &program,
                &["up"],
                true,
                true,
                false,
            )
            .await?;
        if options.execute {
            report = network::detect(&options.admin_url, None).await;
            if !report.tailscale_ready() {
                bail!("Tailscale is still not running with a DNS name after `tailscale up`");
            }
        }
    }

    let hostname = resolve_hostname(options, report.tailscale.dns_name.clone())?;
    let plan = build_plan(NetworkMode::Tailscale, hostname, None, &options.admin_url);
    network::print_plan(&plan);

    runner
        .command(
            "Publish the admin over the tailnet with Tailscale Serve",
            "tailscale",
            &["serve", "--bg", "--https=443", &plan.local_url],
            true,
            true,
            false,
        )
        .await?;

    if options.execute {
        if !options.yes && !confirm("Write admin/daemon/CLI configuration?", true)? {
            bail!("configuration was not written");
        }
        write_configs(options, &plan)?;
        verify_public(&plan.public_url).await;
    } else {
        print_preview_configs(options, &plan);
    }

    print_done(&plan, options);
    Ok(())
}

async fn connect_cloudflare(
    options: &ConnectOptions,
    mut report: network::NetworkReport,
) -> Result<()> {
    let runner = Runner::new(options.execute, options.yes);
    let program = network::tool_program("cloudflared");
    if !report.cloudflare.installed {
        if !ensure_tool(options, &runner, Tool::Cloudflare).await? {
            println!("\nPreview only. Re-run with --execute to install and continue.");
            return Ok(());
        }
        report = network::detect(&options.admin_url, None).await;
        if !report.cloudflare.installed {
            bail!("cloudflared is still not available; install it from https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/ and re-run");
        }
    }
    if report.cloudflare.tunnels.is_none() {
        runner
            .command(
                "Authorize cloudflared in the browser (Cloudflare OAuth)",
                &program,
                &["tunnel", "login"],
                false,
                true,
                false,
            )
            .await?;
        if options.execute {
            report = network::detect(&options.admin_url, None).await;
            if report.cloudflare.tunnels.is_none() {
                bail!("cloudflared is still not authenticated after `cloudflared tunnel login`");
            }
        }
    }

    let mut tunnel = find_tunnel(&report);
    if tunnel.is_none() {
        runner
            .command(
                "Create the `fleet` tunnel",
                &program,
                &["tunnel", "create", "fleet"],
                false,
                true,
                false,
            )
            .await?;
        if options.execute {
            report = network::detect(&options.admin_url, None).await;
            tunnel = find_tunnel(&report);
            if tunnel.is_none() {
                bail!("tunnel `fleet` was not found after creation");
            }
        }
    }

    let hostname = resolve_hostname(options, None)?;
    let plan = build_plan(
        NetworkMode::Cloudflare,
        hostname,
        tunnel,
        &options.admin_url,
    );
    network::print_plan(&plan);

    runner
        .command(
            "Route the public hostname to the tunnel",
            &program,
            &["tunnel", "route", "dns", "fleet", &plan.hostname],
            false,
            false,
            true,
        )
        .await?;

    if options.execute {
        if !options.yes && !confirm("Write admin/daemon/CLI configuration?", true)? {
            bail!("configuration was not written");
        }
        write_configs(options, &plan)?;
        install_cloudflared_service(options, &runner, &plan).await?;
        if !options.skip_access {
            configure_access(options, &plan).await?;
        }
        verify_public(&plan.public_url).await;
    } else {
        print_preview_configs(options, &plan);
        if !options.skip_access {
            let allow = if options.access_everyone {
                "everyone"
            } else if options.access_email.is_some() {
                "one email"
            } else if options.access_domain.is_some() {
                "an email domain"
            } else {
                "prompt for identity"
            };
            println!(
                "\n  Cloudflare Access would be created for {} ({allow})",
                plan.hostname
            );
        }
    }

    print_done(&plan, options);
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum Tool {
    Tailscale,
    Cloudflare,
}

async fn ensure_tool(options: &ConnectOptions, runner: &Runner, tool: Tool) -> Result<bool> {
    let (name, url) = match tool {
        Tool::Tailscale => ("tailscale", "https://tailscale.com/download"),
        Tool::Cloudflare => (
            "cloudflared",
            "https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/",
        ),
    };
    if !options.install {
        bail!(
            "{name} is not installed. Install it from {url}, or re-run with --install to use Homebrew on macOS/Linuxbrew."
        );
    }
    if !options.execute {
        println!(
            "\n=> {name} is missing; with --execute this would run `brew {}`",
            brew_package(tool)
        );
        return Ok(false);
    }
    if !command_available("brew") {
        bail!(
            "{name} is not installed and Homebrew was not found. Install Homebrew, then run `brew {}`, or install {name} from {url}.",
            brew_package(tool)
        );
    }
    match tool {
        Tool::Cloudflare => {
            runner
                .command(
                    "Install cloudflared with Homebrew",
                    "brew",
                    &["install", "cloudflared"],
                    false,
                    true,
                    false,
                )
                .await?;
        }
        Tool::Tailscale if cfg!(target_os = "macos") => {
            runner
                .command(
                    "Install the Tailscale app with Homebrew",
                    "brew",
                    &["install", "--cask", "tailscale"],
                    false,
                    true,
                    false,
                )
                .await?;
            println!(
                "\n  Important: open /Applications/Tailscale.app, sign in, and approve the VPN\n  configuration and network-extension prompts. The CLI cannot grant those\n  permissions automatically."
            );
        }
        Tool::Tailscale => {
            bail!(
                "automatic Tailscale installation is only supported with Homebrew on macOS; install it from {url}"
            );
        }
    }
    Ok(true)
}

fn brew_package(tool: Tool) -> &'static str {
    match tool {
        Tool::Tailscale => "install --cask tailscale",
        Tool::Cloudflare => "install cloudflared",
    }
}

fn command_available(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn find_tunnel(report: &network::NetworkReport) -> Option<Tunnel> {
    report.cloudflare.tunnels.as_ref().and_then(|tunnels| {
        tunnels
            .iter()
            .find(|tunnel| tunnel.name == "fleet")
            .cloned()
    })
}

fn resolve_hostname(options: &ConnectOptions, detected: Option<String>) -> Result<String> {
    if let Some(hostname) = &options.hostname {
        return sanitize_hostname(hostname);
    }
    if let Some(hostname) = detected {
        return sanitize_hostname(&hostname);
    }
    if options.yes {
        bail!("--hostname is required in non-interactive mode");
    }
    let answer = prompt("Public hostname (e.g. fleet.example.com): ", None)?;
    sanitize_hostname(&answer)
}

async fn install_cloudflared_service(
    options: &ConnectOptions,
    runner: &Runner,
    plan: &Plan,
) -> Result<()> {
    if options.skip_service {
        println!("skipping service installation (--skip-service)");
        return Ok(());
    }
    if !cfg!(target_os = "linux") {
        println!(
            "automatic service installation is only implemented on Linux; run cloudflared with:\n  cloudflared tunnel --config {} run fleet",
            options.cloudflared_config.display()
        );
        return Ok(());
    }

    runner
        .command(
            "Install the tunnel configuration for the system service",
            "install",
            &[
                "-D",
                "-m",
                "600",
                &options.cloudflared_config.display().to_string(),
                "/etc/cloudflared/config.yml",
            ],
            true,
            true,
            false,
        )
        .await?;
    runner
        .command(
            "Install cloudflared as a system service",
            &network::tool_program("cloudflared"),
            &["service", "install"],
            true,
            true,
            false,
        )
        .await?;
    runner
        .command(
            "Enable and start the service",
            "systemctl",
            &["enable", "--now", "cloudflared"],
            true,
            true,
            false,
        )
        .await?;

    // `tailscale serve` style verification is not needed here; the public URL
    // check after the service starts is the real confirmation.
    let _ = plan;
    Ok(())
}

async fn configure_access(options: &ConnectOptions, plan: &Plan) -> Result<()> {
    let allow = access_choice(options)?;
    let token = resolve_cloudflare_token(options)?;
    if !options.yes && !confirm("Create a Cloudflare Access application and policy?", true)? {
        println!("skipping Cloudflare Access");
        return Ok(());
    }
    let api = CloudflareApi::new(token.clone())?;
    let account = api
        .resolve_account(options.cloudflare_account_id.as_deref())
        .await?;
    println!("  account    {}", account.name);

    let app = match api.find_access_app(&account.id, &plan.hostname).await? {
        Some(app) => {
            println!(
                "  access app {} ({}) already exists; reusing it",
                app.id, app.domain
            );
            if api.policy_count(&account.id, &app.id).await? == 0 {
                api.create_policy(&account.id, &app.id, &allow).await?;
                println!("  access policy created for {}", allow.description());
            }
            app
        }
        None => {
            let app = api.create_access_app(&account.id, &plan.hostname).await?;
            println!("  access app {} ({}) created", app.id, app.domain);
            api.create_policy(&account.id, &app.id, &allow).await?;
            println!("  access policy created for {}", allow.description());
            app
        }
    };
    println!("  protect    https://{}/ (app {})", plan.hostname, app.id);

    match secrets::store(secrets::CLOUDFLARE_API_TOKEN, &token) {
        Ok(true) => println!("  token      stored in the OS keyring"),
        Ok(false) => println!(
            "  token      stored in {} (0600 fallback)",
            secrets::fallback_path().display()
        ),
        Err(error) => println!("  token      not stored: {error}"),
    }
    Ok(())
}

fn access_choice(options: &ConnectOptions) -> Result<AccessAllow> {
    if options.access_everyone {
        return Ok(AccessAllow::Everyone);
    }
    if let Some(email) = &options.access_email {
        return Ok(AccessAllow::Email(email.clone()));
    }
    if let Some(domain) = &options.access_domain {
        return Ok(AccessAllow::EmailDomain(domain.clone()));
    }
    if options.yes {
        bail!("pass --access-email, --access-domain or --access-everyone (or --skip-access)");
    }
    println!("\nProtect the web UI with Cloudflare Access:");
    println!("  1) allow one email");
    println!("  2) allow an email domain");
    println!("  3) allow everyone (use only with another access control)");
    let choice = prompt("Choice [1/2/3]: ", None)?;
    match choice.trim() {
        "1" => Ok(AccessAllow::Email(prompt("Email: ", None)?)),
        "2" => Ok(AccessAllow::EmailDomain(prompt("Email domain: ", None)?)),
        "3" => Ok(AccessAllow::Everyone),
        _ => bail!("invalid Cloudflare Access choice"),
    }
}

fn resolve_cloudflare_token(options: &ConnectOptions) -> Result<String> {
    if let Some(token) = options
        .cloudflare_api_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        return Ok(token.to_string());
    }
    if let Ok(token) = std::env::var("CLOUDFLARE_API_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }
    if let Some(token) = secrets::load(secrets::CLOUDFLARE_API_TOKEN) {
        return Ok(token);
    }
    if options.yes {
        bail!("a Cloudflare API token is required for Access; pass --cloudflare-api-token or set CLOUDFLARE_API_TOKEN");
    }
    println!(
        "\nCloudflare Access needs an API token with Access: Apps and Policies:Edit,\n\
         Access: Organizations:Read and Account Settings:Read."
    );
    let token = rpassword::prompt_password("Cloudflare API token: ")
        .context("failed to read the Cloudflare API token")?;
    if token.trim().is_empty() {
        bail!("empty Cloudflare API token");
    }
    Ok(token.trim().to_string())
}

fn write_configs(options: &ConnectOptions, plan: &Plan) -> Result<()> {
    let mut wizard = WizardOptions::new(options.admin_url.clone(), None);
    wizard.mode = Some(plan.mode);
    wizard.hostname = Some(plan.hostname.clone());
    wizard.admin_config = options.admin_config.clone();
    wizard.daemon_config = options.daemon_config.clone();
    wizard.cli_config = options.cli_config.clone();
    wizard.cloudflared_config = options.cloudflared_config.clone();
    wizard.yes = true;
    network::apply_plan(plan, &wizard)?;
    Ok(())
}

fn print_preview_configs(options: &ConnectOptions, plan: &Plan) {
    println!("\nConfiguration changes (not written; re-run with --execute):");
    println!(
        "  admin  {}  server.public_url = \"{}\"",
        options.admin_config.display(),
        plan.public_url
    );
    println!(
        "  daemon {}  connection.server_url = \"{}\"",
        options.daemon_config.display(),
        plan.ws_url
    );
    println!(
        "  CLI    {}  public_url = \"{}\"",
        options.cli_config.display(),
        plan.public_url
    );
    if plan.mode == NetworkMode::Cloudflare {
        println!("  tunnel {}", options.cloudflared_config.display());
    }
}

fn print_done(plan: &Plan, options: &ConnectOptions) {
    if !options.execute {
        println!("\nPreview only. Re-run with --execute to apply commands and configuration.");
    } else {
        println!(
            "\nRemote access configured. Verify {} and reconnect daemons with\n  FLEET_SERVER_URL={}",
            plan.public_url, plan.ws_url
        );
    }
}

async fn verify_public(public_url: &str) {
    let url = format!("{public_url}/api/v1/health");
    println!("\nWaiting for {url} …");
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            println!("  could not build HTTP client: {error}");
            return;
        }
    };
    for attempt in 1..=30 {
        match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                println!("  reachable (attempt {attempt})");
                return;
            }
            Ok(response) => {
                if attempt % 5 == 0 {
                    println!("  HTTP {}", response.status());
                }
            }
            Err(error) => {
                if attempt % 5 == 0 {
                    println!("  {error}");
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    println!("  not reachable yet; DNS/TLS may still be propagating. Check {url} manually.");
}

struct Runner {
    execute: bool,
    yes: bool,
}

impl Runner {
    fn new(execute: bool, yes: bool) -> Self {
        Self { execute, yes }
    }

    #[allow(clippy::too_many_arguments)]
    async fn command(
        &self,
        title: &str,
        program: &str,
        args: &[&str],
        sudo: bool,
        required: bool,
        allow_failure: bool,
    ) -> Result<()> {
        let display = format!(
            "{}{} {}",
            if sudo { "sudo " } else { "" },
            program,
            args.join(" ")
        );
        println!("\n=> {title}\n   {display}");
        if !self.execute {
            println!("   (preview; pass --execute to run)");
            return Ok(());
        }
        if !self.yes && !confirm(&format!("Run `{display}`?"), true)? {
            if required {
                bail!("required step skipped: {title}");
            }
            println!("   skipped");
            return Ok(());
        }
        let mut command = if sudo {
            let mut command = Command::new("sudo");
            command.arg(program);
            command
        } else {
            Command::new(program)
        };
        command
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = command
            .status()
            .await
            .with_context(|| format!("failed to start `{display}`"))?;
        if !status.success() {
            if allow_failure {
                println!("   warning: `{display}` exited with {status}; continuing");
                return Ok(());
            }
            bail!("`{display}` exited with {status}");
        }
        Ok(())
    }
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

fn confirm(message: &str, default_yes: bool) -> Result<bool> {
    let answer = prompt(
        &format!("{message} [{}]: ", if default_yes { "Y/n" } else { "y/N" }),
        Some(if default_yes { "y" } else { "n" }),
    )?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        default_admin_config_path, default_cli_config_path, default_daemon_config_path,
    };

    fn options() -> ConnectOptions {
        ConnectOptions {
            provider: NetworkMode::Cloudflare,
            hostname: None,
            admin_url: "http://127.0.0.1:8787".into(),
            admin_config: default_admin_config_path(),
            daemon_config: default_daemon_config_path(),
            cli_config: default_cli_config_path(),
            cloudflared_config: default_cli_config_path(),
            execute: false,
            yes: true,
            cloudflare_api_token: None,
            cloudflare_account_id: None,
            access_email: None,
            access_domain: None,
            access_everyone: false,
            skip_access: false,
            skip_service: false,
            install: false,
        }
    }

    #[test]
    fn resolves_access_choices() {
        let mut opts = options();
        opts.access_everyone = true;
        assert!(matches!(
            access_choice(&opts).unwrap(),
            AccessAllow::Everyone
        ));

        opts.access_everyone = false;
        opts.access_email = Some("ada@example.com".into());
        assert!(matches!(
            access_choice(&opts).unwrap(),
            AccessAllow::Email(_)
        ));

        opts.access_email = None;
        opts.access_domain = Some("example.com".into());
        assert!(matches!(
            access_choice(&opts).unwrap(),
            AccessAllow::EmailDomain(_)
        ));
    }

    #[test]
    fn hostname_prefers_flag_then_detection() {
        let mut opts = options();
        opts.hostname = Some("flag.example.com".into());
        assert_eq!(
            resolve_hostname(&opts, Some("detected.example.com".into())).unwrap(),
            "flag.example.com"
        );
        opts.hostname = None;
        assert_eq!(
            resolve_hostname(&opts, Some("detected.example.com".into())).unwrap(),
            "detected.example.com"
        );
    }
}
