//! Locate the macOS `Fleet.app` wrapper installed by the release installer.
//!
//! The bundle is not a user-facing application; it anchors the launchd jobs to
//! a display name so System Settings -> General -> Login Items & Extensions
//! shows "Fleet" instead of the Developer ID certificate name.
//!
//! Compiled on every host because the launchd plist renderer is testable
//! without a Mac; only macOS uses the path helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::home_dir;

/// Bundle identifier shared by the app bundle and its launchd jobs.
pub const BUNDLE_IDENTIFIER: &str = "dev.fleet.app";

/// Bundle name used by the release archive and the installer.
pub const APP_NAME: &str = "Fleet.app";

/// Every install location the release installer uses, most preferred first.
///
/// The release installer uses `/Applications` and falls back to
/// `<prefix>/share/fleet/Fleet.app`, where `<prefix>` is `~/.local` for a
/// default user install. The running binary's prefix is checked first, then
/// the default `~/.local` prefix for a binary invoked from elsewhere.
pub fn candidates() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/Applications").join(APP_NAME)];
    if let Some(prefix) = install_prefix() {
        push_unique(
            &mut paths,
            prefix.join("share").join("fleet").join(APP_NAME),
        );
    }
    push_unique(
        &mut paths,
        home_dir()
            .join(".local")
            .join("share")
            .join("fleet")
            .join(APP_NAME),
    );
    paths
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn install_prefix() -> Option<PathBuf> {
    install_prefix_from_exe(&std::env::current_exe().ok()?)
}

fn install_prefix_from_exe(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if bin.file_name().and_then(|name| name.to_str()) != Some("bin") {
        return None;
    }
    bin.parent().map(Path::to_path_buf)
}

/// The installed app bundle, if one exists in a standard location.
pub fn installed_path() -> Option<PathBuf> {
    candidates().into_iter().find(|path| path.is_dir())
}

/// Every installed app bundle in a standard location.
pub fn installed_paths() -> Vec<PathBuf> {
    candidates()
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

/// Register an installed app bundle with LaunchServices.
pub fn register_installed() {
    if let Some(app) = installed_path() {
        register(&app);
    }
}

/// Ask LaunchServices to index a bundle. Best effort: the Login Items UI falls
/// back to the signing identity when the bundle cannot be resolved.
pub fn register(path: &Path) {
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/\
                      Frameworks/LaunchServices.framework/Support/lsregister";
    if !Path::new(lsregister).is_file() {
        return;
    }
    let _ = Command::new(lsregister)
        .arg("-f")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_cover_system_and_prefix_share() {
        let paths = candidates();
        assert_eq!(paths[0], PathBuf::from("/Applications/Fleet.app"));
        assert!(paths
            .iter()
            .any(|path| path.ends_with("share/fleet/Fleet.app")));
        assert!(paths.iter().all(|path| path.ends_with(APP_NAME)));
        let mut unique = paths.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), paths.len());
    }

    #[test]
    fn derives_prefix_only_from_a_bin_directory() {
        assert_eq!(
            install_prefix_from_exe(Path::new("/opt/fleet/bin/fleet")),
            Some(PathBuf::from("/opt/fleet"))
        );
        assert_eq!(
            install_prefix_from_exe(Path::new("/opt/fleet/Fleet.app/Contents/MacOS/fleet")),
            None
        );
    }
}
