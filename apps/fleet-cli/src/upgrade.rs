//! Self-upgrade from a published release archive.
//!
//! The upgrade replaces the three binaries and the web assets next to the
//! running `fleet` binary. It verifies `SHA256SUMS`, preserves configuration
//! and data, and restarts installed services.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::config_paths::{Role, RolePathOverrides, RolePaths};
#[cfg(target_os = "macos")]
use crate::macos_app;
use crate::process::{health, service};
use crate::prompt::{self, PromptOptions};

const DEFAULT_RELEASES: &str = "https://github.com/luozhouyang/fleet/releases";

#[derive(Debug, Clone)]
pub struct UpgradeOptions {
    pub version: String,
    pub force: bool,
    pub no_restart: bool,
    pub yes: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Layout {
    bin_dir: PathBuf,
    web_dir: PathBuf,
}

pub async fn run(options: UpgradeOptions) -> Result<()> {
    let version = options.version.trim().to_string();
    validate_version(&version)?;
    if version != "latest"
        && version.trim_start_matches('v') == env!("CARGO_PKG_VERSION")
        && !options.force
    {
        println!("already running {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let exe = std::env::current_exe().context("could not locate the running fleet binary")?;
    let layout = layout_for_exe(&exe)?;
    let asset = platform_asset(std::env::consts::OS, std::env::consts::ARCH)?;
    let base = release_base(&version)?;
    let archive_url = format!("{base}/{asset}");
    let sums_url = format!("{base}/SHA256SUMS");

    if !options.dry_run
        && !options.yes
        && !(PromptOptions {
            yes: false,
            dry_run: false,
        })
        .interactive()
    {
        bail!("pass --yes to upgrade without an interactive terminal");
    }
    if !options.dry_run
        && !prompt::confirm(
            PromptOptions {
                yes: options.yes,
                dry_run: false,
            },
            &format!(
                "Upgrade Fleet to {version} in {}?",
                layout.bin_dir.display()
            ),
            true,
        )?
    {
        return Ok(());
    }

    if options.dry_run {
        println!("release:  {version}");
        println!("asset:    {archive_url}");
        println!("checksums:{sums_url}");
        println!("binaries: {}", layout.bin_dir.display());
        println!("web:      {}", layout.web_dir.display());
        if !options.no_restart {
            println!("services: installed native services would be restarted");
        }
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("fleet/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let sums = fetch_text(&client, &sums_url).await?;
    let expected = expected_checksum(&sums, &asset)
        .with_context(|| format!("{asset} is missing from SHA256SUMS"))?;
    let archive = fetch_bytes(&client, &archive_url).await?;
    let actual = sha256_bytes(&archive);
    if !actual.eq_ignore_ascii_case(&expected) {
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}");
    }

    let work = tempfile::tempdir().context("failed to create a temporary directory")?;
    let archive_path = work.path().join(&asset);
    fs::write(&archive_path, &archive)
        .with_context(|| format!("failed to write {}", archive_path.display()))?;
    let extract_dir = work.path().join("extract");
    fs::create_dir_all(&extract_dir)?;
    extract_archive(&archive_path, &extract_dir)?;

    let mut backups = Backups::default();
    let replaced = replace_binaries(&extract_dir, &layout, &mut backups).and_then(|_| {
        replace_web(&extract_dir, &layout, &mut backups)?;
        Ok(())
    });
    if let Err(err) = replaced {
        let _ = backups.restore();
        return Err(err).context("failed to replace the installed files; rolled back");
    }
    #[cfg(target_os = "macos")]
    if let Err(err) = replace_app(&extract_dir) {
        // The app bundle only anchors the Login Items name. A root-owned
        // /Applications install must not fail a user-scope binary upgrade.
        eprintln!("warning: could not update the Fleet.app bundle: {err}");
    }
    println!("installed Fleet {version} in {}", layout.bin_dir.display());

    if !options.no_restart {
        let restart = restart_services().await;
        let ready = match &restart {
            Ok(()) => wait_services_ready(Duration::from_secs(45)).await,
            Err(err) => Err(anyhow::anyhow!("{err}")),
        };
        if let Err(err) = ready {
            eprintln!("error: {err}");
            eprintln!("rolling back to the previous binaries");
            backups.restore()?;
            let _ = restart_services().await;
            bail!("upgrade to {version} failed; restored the previous installation");
        }
        backups.discard();
    } else {
        println!("skipped service restart; restart running roles manually");
        backups.discard();
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<()> {
    if version == "latest" {
        return Ok(());
    }
    if !version.starts_with('v')
        || version.len() < 2
        || !version[1..]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        bail!("invalid version `{version}`; use `latest` or a tag such as v0.1.0");
    }
    Ok(())
}

fn release_base(version: &str) -> Result<String> {
    if let Ok(base) = std::env::var("FLEET_BASE_URL") {
        let base = base.trim().trim_end_matches('/');
        if !base.is_empty() {
            return Ok(base.to_string());
        }
    }
    if version == "latest" {
        Ok(format!("{DEFAULT_RELEASES}/latest/download"))
    } else {
        Ok(format!("{DEFAULT_RELEASES}/download/{version}"))
    }
}

fn platform_asset(os: &str, arch: &str) -> Result<String> {
    let os = match os {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        other => bail!("unsupported operating system for upgrades: {other}"),
    };
    let arch = match arch {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "arm64",
        other => bail!("unsupported architecture for upgrades: {other}"),
    };
    Ok(if os == "windows" {
        format!("fleet-windows-{arch}.zip")
    } else {
        format!("fleet-{os}-{arch}.tar.gz")
    })
}

fn layout_for_exe(exe: &Path) -> Result<Layout> {
    let bin_dir = exe
        .parent()
        .context("the fleet binary has no parent directory")?
        .to_path_buf();
    let prefix = bin_dir
        .parent()
        .context("the fleet binary is not inside a bin directory")?;
    let share = prefix.join("share").join("fleet").join("web");
    let sibling = prefix.join("web");
    let web_dir = if share.is_dir() {
        share
    } else if sibling.is_dir() {
        sibling
    } else {
        share
    };
    Ok(Layout { bin_dir, web_dir })
}

fn binary_names() -> [&'static str; 3] {
    if cfg!(windows) {
        ["fleet.exe", "fleet-admin.exe", "fleet-daemon.exe"]
    } else {
        ["fleet", "fleet-admin", "fleet-daemon"]
    }
}

/// Paths moved aside during an upgrade so a failed health check can restore
/// the previous installation.
#[derive(Default)]
struct Backups {
    files: Vec<(PathBuf, PathBuf)>,
    web: Option<(PathBuf, PathBuf)>,
}

impl Backups {
    fn discard(&self) {
        for (_, backup) in &self.files {
            let _ = fs::remove_file(backup);
        }
        if let Some((_, backup)) = &self.web {
            let _ = fs::remove_dir_all(backup);
        }
    }

    fn restore(&self) -> Result<()> {
        for (original, backup) in self.files.iter().rev() {
            if !backup.exists() {
                continue;
            }
            let _ = fs::remove_file(original);
            fs::rename(backup, original)
                .with_context(|| format!("failed to restore {}", original.display()))?;
        }
        if let Some((original, backup)) = &self.web {
            if backup.exists() {
                let _ = fs::remove_dir_all(original);
                fs::rename(backup, original)
                    .with_context(|| format!("failed to restore {}", original.display()))?;
            }
        }
        Ok(())
    }
}

fn replace_binaries(extracted: &Path, layout: &Layout, backups: &mut Backups) -> Result<()> {
    let source = extracted.join("bin");
    for name in binary_names() {
        let from = source.join(name);
        if !from.is_file() {
            bail!("{} is missing from the release archive", from.display());
        }
        let to = layout.bin_dir.join(name);
        replace_file(&from, &to, backups)?;
    }
    Ok(())
}

fn replace_file(from: &Path, to: &Path, backups: &mut Backups) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = to.with_file_name(format!(
        "{}.new-{}",
        to.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    fs::copy(from, &temp)
        .with_context(|| format!("failed to copy {} to {}", from.display(), temp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o755))?;
    }

    if to.exists() {
        let backup = to.with_file_name(format!(
            "{}.old-{}",
            to.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        let _ = fs::remove_file(&backup);
        fs::rename(to, &backup)
            .with_context(|| format!("failed to move the old {}", to.display()))?;
        backups.files.push((to.to_path_buf(), backup));
    }
    if let Err(err) = fs::rename(&temp, to) {
        let _ = fs::remove_file(&temp);
        return Err(err).with_context(|| format!("failed to install {}", to.display()));
    }
    Ok(())
}

fn replace_web(extracted: &Path, layout: &Layout, backups: &mut Backups) -> Result<()> {
    let source = extracted.join("web");
    if !source.is_dir() {
        return Ok(());
    }
    if layout.web_dir.exists() {
        let backup = layout.web_dir.with_file_name(format!(
            "{}.old-{}",
            layout
                .web_dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&backup);
        fs::rename(&layout.web_dir, &backup).with_context(|| {
            format!(
                "failed to move the old web directory {}",
                layout.web_dir.display()
            )
        })?;
        backups.web = Some((layout.web_dir.clone(), backup));
    }
    if fs::rename(&source, &layout.web_dir).is_err() {
        copy_dir(&source, &layout.web_dir)?;
    }
    Ok(())
}

/// Replace the installed Fleet.app bundle when the release contains one.
#[cfg(target_os = "macos")]
fn replace_app(extracted: &Path) -> Result<()> {
    let source = extracted.join(macos_app::APP_NAME);
    if !source.is_dir() {
        return Ok(());
    }
    let Some(dest) = macos_app::installed_path() else {
        return Ok(());
    };
    let parent = dest
        .parent()
        .context("the Fleet.app bundle has no parent directory")?;
    let name = dest
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let staging = parent.join(format!(".{name}.new-{}", std::process::id()));
    let backup = parent.join(format!(".{name}.old-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_dir_all(&backup);
    copy_dir(&source, &staging)
        .with_context(|| format!("failed to stage {}", staging.display()))?;
    if let Err(err) = fs::rename(&dest, &backup) {
        let _ = fs::remove_dir_all(&staging);
        return Err(err).with_context(|| format!("failed to move the old {}", dest.display()));
    }
    if let Err(err) = fs::rename(&staging, &dest) {
        let _ = fs::rename(&backup, &dest);
        let _ = fs::remove_dir_all(&staging);
        return Err(err).with_context(|| format!("failed to install {}", dest.display()));
    }
    let _ = fs::remove_dir_all(&backup);
    macos_app::register(&dest);
    println!("updated {}", dest.display());
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn extract_archive(archive: &Path, into: &Path) -> Result<()> {
    let status = if cfg!(windows) {
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                    archive.display().to_string().replace('\'', "''"),
                    into.display().to_string().replace('\'', "''")
                ),
            ])
            .status()
            .context("failed to run powershell.exe")?
    } else {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(into)
            .status()
            .context("failed to run tar")?
    };
    if !status.success() {
        bail!("failed to extract {}", archive.display());
    }
    Ok(())
}

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("download failed for {url}"))?;
    Ok(response.bytes().await?.to_vec())
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let bytes = fetch_bytes(client, url).await?;
    String::from_utf8(bytes).with_context(|| format!("{url} is not UTF-8"))
}

fn expected_checksum(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset).then(|| hash.to_string())
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

async fn restart_services() -> Result<()> {
    let mut errors = Vec::new();
    for role in [Role::Admin, Role::Daemon] {
        let paths = match RolePaths::resolve_config(role, &RolePathOverrides::default()) {
            Ok(paths) => paths,
            Err(_) => continue,
        };
        if !service::installed(&paths) {
            continue;
        }
        if let Err(err) = service::restart(&paths) {
            errors.push(format!("{}: {err}", role.as_str()));
        } else {
            println!(
                "restarted {} {}",
                role.as_str(),
                service::active_name_for(&paths)
            );
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(
            "binaries were upgraded but some services did not restart: {}",
            errors.join("; ")
        )
    }
}

/// Wait until every installed service answers its readiness probe.
async fn wait_services_ready(limit: Duration) -> Result<()> {
    let mut pending: Vec<(String, String)> = Vec::new();
    for role in [Role::Admin, Role::Daemon] {
        let Ok(paths) = RolePaths::resolve_config(role, &RolePathOverrides::default()) else {
            continue;
        };
        if !service::installed(&paths) {
            continue;
        }
        let url = health::health_url(&paths)?;
        let url = if role == Role::Admin {
            url.replace("/api/v1/health", "/api/v1/ready")
        } else {
            url
        };
        pending.push((role.as_str().to_string(), url));
    }
    if pending.is_empty() {
        return Ok(());
    }

    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        let mut still_pending = Vec::new();
        for (label, url) in pending {
            if !health::probe_url(&url).await {
                still_pending.push((label, url));
            }
        }
        pending = still_pending;
        if pending.is_empty() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let detail = pending
        .iter()
        .map(|(label, url)| format!("{label} ({url})"))
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "services did not become ready within {}s: {detail}",
        limit.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_release_assets() {
        assert_eq!(
            platform_asset("linux", "x86_64").unwrap(),
            "fleet-linux-x86_64.tar.gz"
        );
        assert_eq!(
            platform_asset("linux", "aarch64").unwrap(),
            "fleet-linux-arm64.tar.gz"
        );
        assert_eq!(
            platform_asset("macos", "aarch64").unwrap(),
            "fleet-darwin-arm64.tar.gz"
        );
        assert_eq!(
            platform_asset("windows", "x86_64").unwrap(),
            "fleet-windows-x86_64.zip"
        );
        assert!(platform_asset("plan9", "x86_64").is_err());
    }

    #[test]
    fn validates_versions() {
        assert!(validate_version("latest").is_ok());
        assert!(validate_version("v0.1.0").is_ok());
        assert!(validate_version("0.1.0").is_err());
        assert!(validate_version("v../../etc").is_err());
    }

    #[test]
    fn parses_sha256sums() {
        let sums = "abc  fleet-linux-x86_64.tar.gz\ndef  *fleet-windows-x86_64.zip\n";
        assert_eq!(
            expected_checksum(sums, "fleet-linux-x86_64.tar.gz").as_deref(),
            Some("abc")
        );
        assert_eq!(
            expected_checksum(sums, "fleet-windows-x86_64.zip").as_deref(),
            Some("def")
        );
        assert_eq!(expected_checksum(sums, "missing"), None);
    }

    #[test]
    fn upgrades_binaries_in_a_fake_prefix() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let web = root.path().join("web");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("index.html"), "old").unwrap();
        for name in binary_names() {
            std::fs::write(bin.join(name), "old").unwrap();
        }

        let extracted = root.path().join("extract");
        std::fs::create_dir_all(extracted.join("bin")).unwrap();
        std::fs::create_dir_all(extracted.join("web")).unwrap();
        std::fs::write(extracted.join("web/index.html"), "new").unwrap();
        for name in binary_names() {
            std::fs::write(extracted.join("bin").join(name), "new").unwrap();
        }

        let layout = layout_for_exe(&bin.join(binary_names()[0])).unwrap();
        let mut backups = Backups::default();
        replace_binaries(&extracted, &layout, &mut backups).unwrap();
        replace_web(&extracted, &layout, &mut backups).unwrap();

        assert_eq!(
            std::fs::read_to_string(bin.join(binary_names()[0])).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(layout.web_dir.join("index.html")).unwrap(),
            "new"
        );
    }

    #[test]
    fn failed_upgrade_restores_binaries_and_web() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let web = root.path().join("web");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("index.html"), "old").unwrap();
        for name in binary_names() {
            std::fs::write(bin.join(name), "old").unwrap();
        }

        let extracted = root.path().join("extract");
        std::fs::create_dir_all(extracted.join("bin")).unwrap();
        std::fs::create_dir_all(extracted.join("web")).unwrap();
        std::fs::write(extracted.join("web/index.html"), "new").unwrap();
        for name in binary_names() {
            std::fs::write(extracted.join("bin").join(name), "new").unwrap();
        }

        let layout = layout_for_exe(&bin.join(binary_names()[0])).unwrap();
        let mut backups = Backups::default();
        replace_binaries(&extracted, &layout, &mut backups).unwrap();
        replace_web(&extracted, &layout, &mut backups).unwrap();
        backups.restore().unwrap();

        for name in binary_names() {
            assert_eq!(
                std::fs::read_to_string(bin.join(name)).unwrap(),
                "old",
                "{name} was not restored"
            );
        }
        assert_eq!(
            std::fs::read_to_string(layout.web_dir.join("index.html")).unwrap(),
            "old"
        );
    }
}
