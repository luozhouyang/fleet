//! Per-machine token and stable machine-id files stored next to the key.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{AuthError, Result};
#[cfg(unix)]
use crate::identity::check_permissions;
use crate::identity::MachineIdentity;

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

/// `<key_file>.token`
pub fn machine_token_path(key_file: &Path) -> PathBuf {
    with_suffix(key_file, ".token")
}

/// `<key_file>.id`
pub fn machine_id_path(key_file: &Path) -> PathBuf {
    with_suffix(key_file, ".id")
}

/// Load the per-machine token, rejecting group/world-readable files.
pub fn load_machine_token(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    #[cfg(unix)]
    check_permissions(path)?;

    let raw = fs::read_to_string(path).map_err(|err| AuthError::io(path, err))?;
    let token = raw.trim();
    if token.is_empty() {
        Ok(None)
    } else {
        Ok(Some(token.to_string()))
    }
}

/// Write the token with mode 0600 and a trailing newline.
pub fn save_machine_token(path: &Path, token: &str) -> Result<()> {
    write_private(path, format!("{}\n", token.trim()).as_bytes())
}

/// Remove the token file if it exists.
pub fn clear_machine_token(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AuthError::io(path, err)),
    }
}

/// Read `<key_file>.id`, creating it from the key fingerprint when absent.
pub fn load_or_create_machine_id(key_file: &Path, identity: &MachineIdentity) -> Result<String> {
    let path = machine_id_path(key_file);
    if path.exists() {
        let value = fs::read_to_string(&path).map_err(|err| AuthError::io(&path, err))?;
        let value = value.trim();
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }
    let value = identity.fingerprint();
    write_private(&path, format!("{value}\n").as_bytes())?;
    Ok(value)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| AuthError::io(parent, err))?;
        }
    }

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut handle = options.open(path).map_err(|err| AuthError::io(path, err))?;
    handle
        .write_all(bytes)
        .map_err(|err| AuthError::io(path, err))?;
    handle.flush().map_err(|err| AuthError::io(path, err))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|err| AuthError::io(path, err))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_and_machine_id_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("machine.key");
        let token_path = machine_token_path(&key_file);
        assert!(load_machine_token(&token_path).unwrap().is_none());

        save_machine_token(&token_path, "flm_abc").unwrap();
        assert_eq!(
            load_machine_token(&token_path).unwrap().as_deref(),
            Some("flm_abc")
        );

        clear_machine_token(&token_path).unwrap();
        assert!(load_machine_token(&token_path).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn saved_token_is_owner_only_and_world_readable_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("machine.key");
        let token_path = machine_token_path(&key_file);
        save_machine_token(&token_path, "flm_abc").unwrap();

        let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_machine_token(&token_path).is_err());
    }

    #[test]
    fn machine_id_is_stable_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("machine.key");
        let identity = crate::MachineIdentity::generate();
        let first = load_or_create_machine_id(&key_file, &identity).unwrap();
        let second = load_or_create_machine_id(&key_file, &identity).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("m_"));
    }
}
