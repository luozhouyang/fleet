use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::config::{expand_tilde, home_dir};

/// A fleet service role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    Daemon,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Daemon => "daemon",
        }
    }

    pub fn binary_name(self) -> &'static str {
        match self {
            Role::Admin => "fleet-admin",
            Role::Daemon => "fleet-daemon",
        }
    }

    pub fn unit_name(self) -> &'static str {
        match self {
            Role::Admin => "fleet-admin.service",
            Role::Daemon => "fleet-daemon.service",
        }
    }

    pub fn bin_env(self) -> &'static str {
        match self {
            Role::Admin => "FLEET_ADMIN_BIN",
            Role::Daemon => "FLEET_DAEMON_BIN",
        }
    }
}

/// Where a role's service definition is installed.
///
/// Fleet ships a single server profile: services start at boot and run as the
/// invoking user, while configuration and state stay in that user's home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceLevel {
    System,
}

impl ServiceLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceLevel::System => "system",
        }
    }
}

/// The identity a service runs as. Only the installing user is supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAs {
    CurrentUser,
}

impl RunAs {
    pub fn as_str(self) -> &'static str {
        match self {
            RunAs::CurrentUser => "user",
        }
    }
}

/// Explicit per-path overrides supplied on the command line.
#[derive(Debug, Clone, Default)]
pub struct RolePathOverrides {
    pub config: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub database: Option<PathBuf>,
    pub bin: Option<PathBuf>,
}

/// Fully resolved paths and service identity for one role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolePaths {
    pub role: Role,
    pub config: PathBuf,
    pub state_dir: PathBuf,
    pub database: PathBuf,
    /// Resolved server binary. `None` when only configuration paths are needed.
    pub binary: Option<PathBuf>,
    /// The `fleet` binary that owns the role commands.
    pub fleet_binary: PathBuf,
    /// User the service runs as. Always the invoking user.
    pub service_user: String,
    pub service_group: String,
    pub web_dir: Option<PathBuf>,
}

impl RolePaths {
    /// Resolve paths and discover the server binary.
    pub fn resolve(role: Role, overrides: &RolePathOverrides) -> Result<Self> {
        Self::resolve_inner(
            role,
            overrides,
            &current_user(),
            &current_group(),
            &home_dir(),
            true,
        )
    }

    /// Resolve configuration paths without discovering the server binary.
    pub fn resolve_config(role: Role, overrides: &RolePathOverrides) -> Result<Self> {
        Self::resolve_inner(
            role,
            overrides,
            &current_user(),
            &current_group(),
            &home_dir(),
            false,
        )
    }

    #[cfg(test)]
    pub fn resolve_with(
        role: Role,
        overrides: &RolePathOverrides,
        user: &str,
        group: &str,
        home: &Path,
    ) -> Result<Self> {
        Self::resolve_inner(role, overrides, user, group, home, true)
    }

    fn resolve_inner(
        role: Role,
        overrides: &RolePathOverrides,
        user: &str,
        group: &str,
        home: &Path,
        discover: bool,
    ) -> Result<Self> {
        let default_config = match role {
            Role::Admin => fleet_core_config::default_admin_config_path(),
            Role::Daemon => fleet_core_config::default_daemon_config_path(),
        };
        let default_state = home.join(".fleet").join(role.as_str());

        let config = overrides.config.clone().unwrap_or(default_config);
        let state_dir = overrides.state_dir.clone().unwrap_or(default_state);
        let database = overrides.database.clone().unwrap_or_else(|| {
            state_dir.join(match role {
                Role::Admin => "fleet-admin.db",
                Role::Daemon => "fleet-daemon.db",
            })
        });
        let binary = if discover {
            Some(discover_binary(role, overrides.bin.clone())?)
        } else {
            None
        };
        let fleet_binary = std::env::current_exe()?;
        let web_dir = match role {
            Role::Admin => default_web_dir(),
            Role::Daemon => None,
        };

        Ok(Self {
            role,
            config,
            state_dir,
            database,
            binary,
            fleet_binary,
            service_user: user.to_string(),
            service_group: group.to_string(),
            web_dir,
        })
    }

    /// Return the resolved server binary or a clear error.
    pub fn require_binary(&self) -> Result<&Path> {
        self.binary
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("{} binary was not resolved", self.role.binary_name()))
    }

    /// Service definitions always live in the system service manager.
    pub fn service_level(&self) -> ServiceLevel {
        ServiceLevel::System
    }

    /// Services always run as the invoking user, never as root.
    pub fn run_as(&self) -> RunAs {
        RunAs::CurrentUser
    }

    /// Fallback-supervisor state directory.
    pub fn run_dir(&self) -> PathBuf {
        self.state_dir.join("run")
    }

    /// Fallback-supervisor log directory.
    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }

    /// `run/supervisor.json`, written atomically by the supervisor.
    pub fn supervisor_state(&self) -> PathBuf {
        self.run_dir().join("supervisor.json")
    }

    /// `run/start.lock`, held while starting or stopping the supervisor.
    pub fn start_lock(&self) -> PathBuf {
        self.run_dir().join("start.lock")
    }

    /// Server stdout/stderr captured by the fallback supervisor.
    pub fn server_log(&self) -> PathBuf {
        self.logs_dir().join(format!("{}.log", self.role.as_str()))
    }

    /// Supervisor stdout/stderr.
    pub fn supervisor_log(&self) -> PathBuf {
        self.logs_dir().join("supervisor.log")
    }
}

pub fn ensure_distinct_databases(admin: &Path, daemon: &Path) -> Result<()> {
    let admin = normalized(admin);
    let daemon = normalized(daemon);
    if admin == daemon {
        bail!(
            "admin and daemon databases must be different; both resolve to {}",
            admin.display()
        );
    }
    Ok(())
}

fn normalized(path: &Path) -> PathBuf {
    let path = expand_tilde(path.to_path_buf());
    if path.exists() {
        std::fs::canonicalize(&path).unwrap_or(path)
    } else {
        path
    }
}

fn binary_file_name(role: Role) -> String {
    if cfg!(windows) {
        format!("{}.exe", role.binary_name())
    } else {
        role.binary_name().to_string()
    }
}

pub fn discover_binary(role: Role, override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return ensure_executable(path, role);
    }
    if let Some(path) = std::env::var_os(role.bin_env()) {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return ensure_executable(path, role);
        }
    }
    let file_name = binary_file_name(role);
    let mut searched = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(&file_name);
            searched.push(candidate.clone());
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(&file_name);
            searched.push(candidate.clone());
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "could not find {}; pass --bin, set {}, or install it next to fleet. searched: {}",
        file_name,
        role.bin_env(),
        searched
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn ensure_executable(path: PathBuf, role: Role) -> Result<PathBuf> {
    if !path.is_file() {
        bail!(
            "{} binary does not exist: {}",
            role.binary_name(),
            path.display()
        );
    }
    Ok(path)
}

/// Locate the web UI for a release or source layout.
///
/// Release archives keep it in `<prefix>/share/fleet/web`; the Windows zip uses
/// `<prefix>/web`. Source checkouts expose `apps/web/dist` from the current
/// working directory.
fn default_web_dir() -> Option<PathBuf> {
    first_web_dir(
        std::env::current_exe().ok().as_deref(),
        std::env::current_dir().ok().as_deref(),
    )
}

fn release_web_dir_at(exe: Option<&Path>) -> Option<PathBuf> {
    let prefix = exe?.parent()?.parent()?;
    [
        prefix.join("share").join("fleet").join("web"),
        prefix.join("web"),
    ]
    .into_iter()
    .find(|candidate| candidate.join("index.html").is_file())
}

fn first_web_dir(exe: Option<&Path>, cwd: Option<&Path>) -> Option<PathBuf> {
    release_web_dir_at(exe).or_else(|| {
        let candidate = cwd?.join("apps").join("web").join("dist");
        candidate.join("index.html").is_file().then_some(candidate)
    })
}

#[cfg(unix)]
pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
pub fn is_root() -> bool {
    false
}

/// Current user, preferring `SUDO_USER` so sudo does not attribute files to root.
pub fn current_user() -> String {
    std::env::var("SUDO_USER")
        .or_else(|_| std::env::var("USER"))
        // Windows PowerShell sets USERNAME, not USER/LOGNAME.
        .or_else(|_| std::env::var("USERNAME"))
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub fn current_group() -> String {
    std::process::Command::new("id")
        .arg("-gn")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(current_user)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides() -> RolePathOverrides {
        RolePathOverrides::default()
    }

    #[test]
    fn paths_live_under_the_user_home() {
        let home = PathBuf::from("/home/dev");
        let bin = std::env::current_exe().unwrap();
        let mut overrides = overrides();
        overrides.bin = Some(bin.clone());

        let admin = RolePaths::resolve_with(Role::Admin, &overrides, "dev", "dev", &home).unwrap();
        assert_eq!(admin.config, fleet_core_config::default_admin_config_path());
        assert_eq!(admin.state_dir, home.join(".fleet/admin"));
        assert_eq!(admin.database, home.join(".fleet/admin/fleet-admin.db"));
        assert_eq!(admin.service_user, "dev");
        assert_eq!(admin.binary.as_deref(), Some(bin.as_path()));

        let daemon =
            RolePaths::resolve_with(Role::Daemon, &overrides, "dev", "dev", &home).unwrap();
        assert_eq!(
            daemon.config,
            fleet_core_config::default_daemon_config_path()
        );
        assert_eq!(daemon.state_dir, home.join(".fleet/daemon"));
        assert_eq!(daemon.database, home.join(".fleet/daemon/fleet-daemon.db"));
    }

    #[test]
    fn server_profile_uses_system_services_and_the_invoking_user() {
        let home = PathBuf::from("/home/dev");
        let mut overrides = overrides();
        overrides.bin = Some(std::env::current_exe().unwrap());
        let paths = RolePaths::resolve_with(Role::Admin, &overrides, "dev", "dev", &home).unwrap();
        assert_eq!(paths.service_level(), ServiceLevel::System);
        assert_eq!(paths.run_as(), RunAs::CurrentUser);
        assert_eq!(paths.service_user, "dev");
        assert_eq!(paths.service_group, "dev");
    }

    #[test]
    fn database_collision_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same.db");
        assert!(ensure_distinct_databases(&path, &path).is_err());
        assert!(ensure_distinct_databases(&path, &dir.path().join("other.db")).is_ok());
    }

    #[test]
    fn overrides_win_over_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let bin = std::env::current_exe().unwrap();
        let overrides = RolePathOverrides {
            config: Some(dir.path().join("custom.toml")),
            state_dir: Some(dir.path().join("state")),
            database: Some(dir.path().join("custom.db")),
            bin: Some(bin),
        };
        let paths = RolePaths::resolve_with(
            Role::Daemon,
            &overrides,
            "dev",
            "dev",
            Path::new("/home/dev"),
        )
        .unwrap();
        assert_eq!(paths.config, dir.path().join("custom.toml"));
        assert_eq!(paths.database, dir.path().join("custom.db"));
    }

    #[test]
    fn resolve_config_skips_binary_discovery() {
        let paths = RolePaths::resolve_config(Role::Admin, &overrides()).unwrap();
        assert!(paths.binary.is_none());
        assert!(paths.require_binary().is_err());
        assert_eq!(paths.fleet_binary, std::env::current_exe().unwrap());
    }

    #[test]
    fn discover_binary_honors_override_and_rejects_missing() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(
            discover_binary(Role::Admin, Some(exe.clone())).unwrap(),
            exe
        );
        assert!(
            discover_binary(Role::Daemon, Some(PathBuf::from("/missing/fleet-daemon"))).is_err()
        );
    }

    #[test]
    fn runtime_paths_live_under_the_state_directory() {
        let dir = tempfile::tempdir().unwrap();
        let overrides = RolePathOverrides {
            state_dir: Some(dir.path().join("state")),
            bin: Some(std::env::current_exe().unwrap()),
            ..RolePathOverrides::default()
        };
        let paths = RolePaths::resolve_with(
            Role::Daemon,
            &overrides,
            "dev",
            "dev",
            Path::new("/home/dev"),
        )
        .unwrap();
        let state = dir.path().join("state");
        assert_eq!(paths.run_dir(), state.join("run"));
        assert_eq!(paths.logs_dir(), state.join("logs"));
        assert_eq!(paths.supervisor_state(), state.join("run/supervisor.json"));
        assert_eq!(paths.start_lock(), state.join("run/start.lock"));
        assert_eq!(paths.server_log(), state.join("logs/daemon.log"));
        assert_eq!(paths.supervisor_log(), state.join("logs/supervisor.log"));
    }

    #[test]
    fn root_detection_matches_euid() {
        #[cfg(unix)]
        assert_eq!(is_root(), unsafe { libc::geteuid() == 0 });
        #[cfg(not(unix))]
        assert!(!is_root());
    }

    #[test]
    fn default_web_dir_finds_release_and_zip_layouts() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path();
        let bin = prefix.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("fleet");
        std::fs::write(&exe, "").unwrap();

        let share_web = prefix.join("share").join("fleet").join("web");
        std::fs::create_dir_all(&share_web).unwrap();
        std::fs::write(share_web.join("index.html"), "").unwrap();
        assert_eq!(first_web_dir(Some(&exe), None), Some(share_web.clone()));

        std::fs::remove_dir_all(&share_web).unwrap();
        let zip_web = prefix.join("web");
        std::fs::create_dir_all(&zip_web).unwrap();
        std::fs::write(zip_web.join("index.html"), "").unwrap();
        assert_eq!(first_web_dir(Some(&exe), None), Some(zip_web));
    }

    #[test]
    fn default_web_dir_falls_back_to_source_tree() {
        let root = tempfile::tempdir().unwrap();
        let web = root.path().join("apps").join("web").join("dist");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("index.html"), "").unwrap();
        assert_eq!(first_web_dir(None, Some(root.path())), Some(web));
    }
}
