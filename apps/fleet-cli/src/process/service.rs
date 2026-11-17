//! Platform-native service manager abstraction.
//!
//! Linux uses systemd, macOS uses launchd, and Windows uses Task Scheduler for
//! both roles. Hosts without a native manager fall back to the detached
//! supervisor (Unix only).

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config_paths::RolePaths;

use super::{launchd, systemd, windows_service, windows_task};

/// Backend selected for the current host.
#[allow(dead_code)] // variants are selected by target cfg
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Systemd,
    Launchd,
    WindowsService,
    WindowsTask,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::WindowsService => "windows-service",
            Self::WindowsTask => "windows-task",
        }
    }

    pub fn prompt_label(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::WindowsService => "Windows service",
            Self::WindowsTask => "Windows Task Scheduler task",
        }
    }
}

/// Snapshot of a native service's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub backend: &'static str,
    pub active: bool,
    pub state: String,
    pub detail: String,
}

/// Result of installing or removing a native service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub unit_path: PathBuf,
    pub env_path: Option<PathBuf>,
    /// True when privileged commands were printed for manual execution instead
    /// of being run.
    pub skipped: bool,
}

/// The native service manager available for this role.
#[allow(unused_variables)]
pub fn available_for(paths: &RolePaths) -> Option<Backend> {
    #[cfg(target_os = "linux")]
    if systemd::systemd_available() {
        return Some(Backend::Systemd);
    }
    #[cfg(target_os = "macos")]
    if launchd::launchd_available() {
        return Some(Backend::Launchd);
    }
    #[cfg(windows)]
    {
        return windows_task::task_available().then_some(Backend::WindowsTask);
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Name of the backend for this role, or `service` when none is available.
pub fn active_name_for(paths: &RolePaths) -> &'static str {
    available_for(paths).map(Backend::name).unwrap_or("service")
}

/// Prompt label for this role's backend.
pub fn prompt_label_for(paths: &RolePaths) -> &'static str {
    available_for(paths)
        .map(Backend::prompt_label)
        .unwrap_or("service")
}

/// True when this role's native service is installed.
pub fn installed(paths: &RolePaths) -> bool {
    match available_for(paths) {
        Some(Backend::Systemd) => systemd::unit_installed(paths),
        Some(Backend::Launchd) => launchd::unit_installed(paths),
        Some(Backend::WindowsService) => windows_service::unit_installed(paths),
        Some(Backend::WindowsTask) => windows_task::unit_installed(paths),
        None => false,
    }
}

pub fn start(paths: &RolePaths) -> Result<()> {
    match backend(paths)? {
        Backend::Systemd => systemd::start(paths),
        Backend::Launchd => launchd::start(paths),
        Backend::WindowsService => windows_service::start(paths),
        Backend::WindowsTask => windows_task::start(paths),
    }
}

pub fn stop(paths: &RolePaths) -> Result<()> {
    match backend(paths)? {
        Backend::Systemd => systemd::stop(paths),
        Backend::Launchd => launchd::stop(paths),
        Backend::WindowsService => windows_service::stop(paths),
        Backend::WindowsTask => windows_task::stop(paths),
    }
}

pub fn restart(paths: &RolePaths) -> Result<()> {
    match backend(paths)? {
        Backend::Systemd => systemd::restart(paths),
        Backend::Launchd => launchd::restart(paths),
        Backend::WindowsService => windows_service::restart(paths),
        Backend::WindowsTask => windows_task::restart(paths),
    }
}

pub fn status(paths: &RolePaths) -> Option<ServiceStatus> {
    match available_for(paths)? {
        Backend::Systemd => {
            let status = systemd::status(paths);
            Some(ServiceStatus {
                backend: "systemd",
                active: status.active,
                state: status.unit_file_state,
                detail: format!("Restart={}", status.restart),
            })
        }
        Backend::Launchd => Some(launchd::status(paths)),
        Backend::WindowsService => Some(windows_service::status(paths)),
        Backend::WindowsTask => Some(windows_task::status(paths)),
    }
}

pub fn logs(paths: &RolePaths, lines: usize, follow: bool) -> Result<()> {
    match backend(paths)? {
        Backend::Systemd => systemd::logs(paths, lines, follow),
        Backend::Launchd => launchd::logs(paths, lines, follow),
        Backend::WindowsService => windows_service::logs(paths, lines, follow),
        Backend::WindowsTask => windows_task::logs(paths, lines, follow),
    }
}

/// Short hint for where the service's logs live.
pub fn log_hint(paths: &RolePaths) -> String {
    match available_for(paths) {
        Some(Backend::Systemd) => format!("journalctl -u {}", paths.role.unit_name()),
        Some(Backend::Launchd) => launchd::log_hint(paths),
        Some(Backend::WindowsService) => windows_service::log_hint(paths),
        Some(Backend::WindowsTask) => windows_task::log_hint(paths),
        None => paths.server_log().display().to_string(),
    }
}

pub fn install(
    paths: &RolePaths,
    admin_token: Option<&str>,
    start: bool,
    dry_run: bool,
    yes: bool,
) -> Result<InstallOutcome> {
    match backend(paths)? {
        Backend::Systemd => {
            let outcome = systemd::install(paths, admin_token, start, dry_run, yes)?;
            Ok(InstallOutcome {
                unit_path: outcome.unit_path,
                env_path: outcome.env_path,
                skipped: outcome.skipped,
            })
        }
        Backend::Launchd => launchd::install(paths, admin_token, start, dry_run, yes),
        Backend::WindowsService => windows_service::install(paths, start, dry_run, yes),
        Backend::WindowsTask => windows_task::install(paths, start, dry_run, yes),
    }
}

pub fn uninstall(paths: &RolePaths, dry_run: bool, yes: bool) -> Result<()> {
    match backend(paths)? {
        Backend::Systemd => systemd::uninstall(paths, dry_run, yes),
        Backend::Launchd => launchd::uninstall(paths, dry_run, yes),
        Backend::WindowsService => windows_service::uninstall(paths, dry_run, yes),
        Backend::WindowsTask => windows_task::uninstall(paths, dry_run, yes),
    }
}

/// Canonical absolute path in the form native service managers expect.
///
/// On Windows `canonicalize` returns a `\\?\` verbatim path, which neither
/// `sc.exe` nor Task Scheduler accepts in `binPath` and `<Command>`.
pub fn native_path(path: &Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        PathBuf::from(strip_verbatim_text(&path.to_string_lossy()))
    }
    #[cfg(not(windows))]
    {
        path
    }
}

#[cfg(any(windows, test))]
fn strip_verbatim_text(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text.to_string()
    }
}

fn backend(paths: &RolePaths) -> Result<Backend> {
    available_for(paths).ok_or_else(|| {
        anyhow::anyhow!(
            "no native service manager is available for the {} role on this platform; use `fleet {} run` in the foreground",
            paths.role.as_str(),
            paths.role.as_str()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_names_and_labels_are_stable() {
        assert_eq!(Backend::Systemd.name(), "systemd");
        assert_eq!(Backend::Launchd.name(), "launchd");
        assert_eq!(Backend::WindowsService.name(), "windows-service");
        assert_eq!(Backend::WindowsTask.name(), "windows-task");
        assert_eq!(Backend::WindowsService.prompt_label(), "Windows service");
        assert_eq!(
            Backend::WindowsTask.prompt_label(),
            "Windows Task Scheduler task"
        );
    }

    #[test]
    fn strips_windows_verbatim_prefixes() {
        assert_eq!(
            strip_verbatim_text(r"\\?\C:\ProgramData\fleet\admin.toml"),
            r"C:\ProgramData\fleet\admin.toml"
        );
        assert_eq!(
            strip_verbatim_text(r"\\?\UNC\server\share\fleet.exe"),
            r"\\server\share\fleet.exe"
        );
        assert_eq!(
            strip_verbatim_text(r"C:\ProgramData\fleet"),
            r"C:\ProgramData\fleet"
        );
    }
}
