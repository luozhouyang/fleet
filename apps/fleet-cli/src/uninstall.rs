//! Remove Fleet's native services, installed binaries, and optionally data.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::cli::ProcessMode;
use crate::config_paths::{system_config_root, Role, RolePathOverrides, RolePaths, ScopeRequest};
#[cfg(target_os = "macos")]
use crate::macos_app;
use crate::process::{self, service};
use crate::prompt::{self, PromptOptions};

#[derive(Debug, Clone)]
pub struct UninstallOptions {
    pub scope: ScopeRequest,
    pub overrides: RolePathOverrides,
    pub delete_data: bool,
    pub yes: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Plan {
    services: Vec<(Role, RolePaths)>,
    supervisors: Vec<(Role, RolePaths)>,
    binaries: Vec<PathBuf>,
    web_dirs: Vec<PathBuf>,
    apps: Vec<PathBuf>,
    data_dirs: Vec<PathBuf>,
}

pub async fn run(options: UninstallOptions) -> Result<()> {
    let admin = RolePaths::resolve_config(Role::Admin, options.scope, &options.overrides)?;
    let daemon = RolePaths::resolve_config(Role::Daemon, options.scope, &options.overrides)?;
    let mut delete_data = options.delete_data;
    let mut plan = build_plan(&admin, &daemon, delete_data)?;

    if !options.dry_run
        && !options.yes
        && !(PromptOptions {
            yes: false,
            dry_run: false,
        })
        .interactive()
    {
        bail!("pass --yes to uninstall without an interactive terminal");
    }
    if !options.dry_run
        && !prompt::confirm(
            PromptOptions {
                yes: options.yes,
                dry_run: false,
            },
            "Remove Fleet services and installed binaries?",
            false,
        )?
    {
        return Ok(());
    }
    if delete_data
        && !options.dry_run
        && !prompt::confirm(
            PromptOptions {
                yes: options.yes,
                dry_run: false,
            },
            &format!(
                "Delete Fleet configuration and data at {}?",
                plan.data_dirs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            false,
        )?
    {
        delete_data = false;
        plan = build_plan(&admin, &daemon, false)?;
        println!("kept Fleet data");
    }

    print_plan(&plan, delete_data, options.dry_run);
    if options.dry_run {
        return Ok(());
    }

    for (role, paths) in &plan.services {
        if service::installed(paths) {
            service::uninstall(paths, false, options.yes)
                .with_context(|| format!("failed to remove the {} service", role.as_str()))?;
        }
    }
    for (role, paths) in &plan.supervisors {
        process::stop_role(paths, ProcessMode::Supervisor, 15)
            .await
            .with_context(|| format!("failed to stop the {} supervisor", role.as_str()))?;
    }
    remove_stale_binaries(&plan.binaries);
    for path in &plan.binaries {
        remove_file(path)?;
    }
    for dir in &plan.web_dirs {
        remove_dir(dir)?;
    }
    for dir in &plan.apps {
        remove_dir(dir)?;
    }
    if delete_data {
        for dir in &plan.data_dirs {
            remove_dir(dir)?;
        }
    }

    println!("Fleet removed");
    if !delete_data && !plan.data_dirs.is_empty() {
        println!(
            "configuration and data were kept at {}",
            plan.data_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

fn build_plan(admin: &RolePaths, daemon: &RolePaths, delete_data: bool) -> Result<Plan> {
    let bin_dir = std::env::current_exe()
        .context("could not locate the running fleet binary")?
        .parent()
        .context("the fleet binary has no parent directory")?
        .to_path_buf();
    let names = binary_names();
    let binaries = names
        .iter()
        .map(|name| bin_dir.join(name))
        .collect::<Vec<_>>();

    // Installed web assets live in `<prefix>/share/fleet/web` for the release
    // installer, or `<prefix>/web` in the Windows zip layout.
    let prefix = bin_dir
        .parent()
        .context("the fleet binary is not inside a bin directory")?;
    let mut web_dirs = Vec::new();
    for candidate in [prefix.join("share").join("fleet"), prefix.join("web")] {
        if candidate.is_dir() {
            web_dirs.push(candidate);
        }
    }
    if web_dirs.is_empty() {
        web_dirs.push(prefix.join("share").join("fleet"));
    }
    let mut data_dirs = Vec::new();
    for paths in [admin, daemon] {
        if let Some(parent) = paths.state_dir.parent() {
            if safe_data_root(parent) && !data_dirs.contains(&parent.to_path_buf()) {
                data_dirs.push(parent.to_path_buf());
            }
        }
    }
    if delete_data {
        let config_root = system_config_root();
        if admin.config.starts_with(&config_root) && !data_dirs.contains(&config_root) {
            data_dirs.push(config_root);
        }
    }

    let roles = [admin, daemon];
    let services = roles
        .iter()
        .filter(|paths| service::installed(paths))
        .map(|paths| (paths.role, (*paths).clone()))
        .collect();
    let supervisors = roles
        .iter()
        .filter(|paths| process::supervisor_live(paths))
        .map(|paths| (paths.role, (*paths).clone()))
        .collect();

    #[cfg(target_os = "macos")]
    let apps: Vec<PathBuf> = macos_app::installed_paths();
    #[cfg(not(target_os = "macos"))]
    let apps: Vec<PathBuf> = Vec::new();

    Ok(Plan {
        services,
        supervisors,
        binaries,
        web_dirs,
        apps,
        data_dirs,
    })
}

fn binary_names() -> [&'static str; 3] {
    if cfg!(windows) {
        ["fleet.exe", "fleet-admin.exe", "fleet-daemon.exe"]
    } else {
        ["fleet", "fleet-admin", "fleet-daemon"]
    }
}

fn safe_data_root(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(name, ".fleet" | "fleet") && path.parent().is_some()
}

fn print_plan(plan: &Plan, delete_data: bool, dry_run: bool) {
    let prefix = if dry_run { "would remove" } else { "removing" };
    for (role, paths) in &plan.services {
        println!(
            "{prefix} {} service ({})",
            role.as_str(),
            service::active_name_for(paths)
        );
    }
    for (role, _) in &plan.supervisors {
        println!("{prefix} {} fallback supervisor", role.as_str());
    }
    for path in &plan.binaries {
        println!("{prefix} {}", path.display());
    }
    for path in &plan.web_dirs {
        println!("{prefix} {}", path.display());
    }
    for path in &plan.apps {
        println!("{prefix} {}", path.display());
    }
    if delete_data {
        for path in &plan.data_dirs {
            println!("{prefix} {}", path.display());
        }
    } else {
        for path in &plan.data_dirs {
            println!("keeping {}", path.display());
        }
    }
}

fn remove_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        // Windows cannot delete the running executable; rename it and let the
        // next cleanup remove the stale file.
        Err(err) if cfg!(windows) => {
            let stale = path.with_file_name(format!(
                "{}.old-{}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                std::process::id()
            ));
            let _ = fs::remove_file(&stale);
            fs::rename(path, &stale).with_context(|| {
                format!(
                    "failed to remove {} ({err}); close Fleet and delete it manually",
                    path.display()
                )
            })?;
            let _ = fs::remove_file(&stale);
            println!("renamed {}; delete it after Fleet exits", path.display());
            Ok(())
        }
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", path.display())),
    }
}

/// Remove `.old-<pid>` files left behind by a previous Windows uninstall.
fn remove_stale_binaries(binaries: &[PathBuf]) {
    for path in binaries {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            continue;
        };
        let prefix = format!("{}.old-", name.to_string_lossy());
        let Ok(entries) = fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn remove_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_paths::Scope;

    fn paths(role: Role, scope: Scope, home: &Path) -> RolePaths {
        let overrides = RolePathOverrides {
            bin: Some(std::env::current_exe().unwrap()),
            ..RolePathOverrides::default()
        };
        RolePaths::resolve_with(role, scope, &overrides, "dev", "dev", home).unwrap()
    }

    #[test]
    fn builds_keep_data_plan_for_user_scope() {
        let home = tempfile::tempdir().unwrap();
        let admin = paths(Role::Admin, Scope::User, home.path());
        let daemon = paths(Role::Daemon, Scope::User, home.path());
        let plan = build_plan(&admin, &daemon, false).unwrap();
        assert!(plan.data_dirs.contains(&home.path().join(".fleet")));
        assert!(plan.binaries.iter().any(|path| path.ends_with("fleet")));
    }

    #[test]
    fn delete_data_includes_data_roots() {
        let home = tempfile::tempdir().unwrap();
        let admin = paths(Role::Admin, Scope::User, home.path());
        let daemon = paths(Role::Daemon, Scope::User, home.path());
        let plan = build_plan(&admin, &daemon, true).unwrap();
        assert!(plan.data_dirs.iter().any(|path| path.ends_with(".fleet")));
    }

    #[test]
    fn system_plan_keeps_config_unless_deleting() {
        let home = tempfile::tempdir().unwrap();
        let admin = paths(Role::Admin, Scope::System, home.path());
        let daemon = paths(Role::Daemon, Scope::System, home.path());

        let keep = build_plan(&admin, &daemon, false).unwrap();
        assert!(keep
            .data_dirs
            .iter()
            .any(|path| path == &PathBuf::from("/var/lib/fleet")));
        assert!(!keep
            .data_dirs
            .iter()
            .any(|path| path == &PathBuf::from("/etc/fleet")));

        let purge = build_plan(&admin, &daemon, true).unwrap();
        assert!(purge
            .data_dirs
            .iter()
            .any(|path| path == &PathBuf::from("/etc/fleet")));
    }

    #[test]
    fn custom_state_dir_is_planned_via_overrides() {
        let home = tempfile::tempdir().unwrap();
        let state = home.path().join("custom/.fleet");
        let overrides = RolePathOverrides {
            state_dir: Some(state.join("admin")),
            bin: Some(std::env::current_exe().unwrap()),
            ..RolePathOverrides::default()
        };
        let admin = RolePaths::resolve_with(
            Role::Admin,
            Scope::User,
            &overrides,
            "dev",
            "dev",
            home.path(),
        )
        .unwrap();
        let mut daemon_overrides = overrides.clone();
        daemon_overrides.state_dir = Some(state.join("daemon"));
        let daemon = RolePaths::resolve_with(
            Role::Daemon,
            Scope::User,
            &daemon_overrides,
            "dev",
            "dev",
            home.path(),
        )
        .unwrap();
        let plan = build_plan(&admin, &daemon, false).unwrap();
        assert_eq!(plan.data_dirs, vec![state]);
    }

    #[test]
    fn refuses_unsafe_data_roots() {
        assert!(!safe_data_root(Path::new("/")));
        assert!(!safe_data_root(Path::new("/home/dev")));
        assert!(safe_data_root(Path::new("/home/dev/.fleet")));
        assert!(safe_data_root(Path::new("/var/lib/fleet")));
    }

    #[test]
    fn removes_stale_binaries_from_previous_runs() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("fleet");
        let stale = dir.path().join("fleet.old-123");
        let unrelated = dir.path().join("fleet-admin");
        std::fs::write(&live, "live").unwrap();
        std::fs::write(&stale, "stale").unwrap();
        std::fs::write(&unrelated, "keep").unwrap();

        remove_stale_binaries(std::slice::from_ref(&live));

        assert!(live.exists());
        assert!(unrelated.exists());
        assert!(!stale.exists());
    }

    #[test]
    fn removes_a_fake_data_directory() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join(".fleet");
        std::fs::create_dir_all(data.join("admin")).unwrap();
        std::fs::write(data.join("admin/config.toml"), "x").unwrap();
        remove_dir(&data).unwrap();
        assert!(!data.exists());
    }
}
