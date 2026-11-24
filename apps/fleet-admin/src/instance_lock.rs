//! Single-instance guard for the admin database.
//!
//! Two admins sharing one SQLite file keep divergent in-memory state — the
//! connection registry, event bus and machine index — so the second process
//! must refuse to start. The guard is an advisory exclusive lock on a sibling
//! file, released when the process exits or the handle closes.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// An exclusive advisory lock held for the lifetime of the process.
#[derive(Debug)]
pub struct InstanceLock {
    /// Held open for the guard's lifetime; closing it releases the lock.
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Acquire the lock for `database`, failing when another process holds it.
    pub fn acquire(database: &Path) -> anyhow::Result<Self> {
        let path = lock_path(database);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file, path }),
            Err(TryLockError::WouldBlock) => anyhow::bail!(
                "another fleet-admin is already using {}; stop it before starting a new one (lock file: {})",
                database.display(),
                path.display()
            ),
            Err(TryLockError::Error(err)) => Err(err.into()),
        }
    }

    /// Path of the lock file, for diagnostics.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Sibling lock file so the database itself keeps its permissions.
fn lock_path(database: &Path) -> PathBuf {
    let mut path = database.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_blocks_a_second_instance() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("admin.db");
        let first = InstanceLock::acquire(&database).unwrap();
        assert_eq!(lock_path(&database), first.path());

        let error = InstanceLock::acquire(&database).unwrap_err();
        assert!(error.to_string().contains("another fleet-admin"));
        drop(first);

        let _second = InstanceLock::acquire(&database).unwrap();
    }

    #[test]
    fn the_lock_creates_the_database_directory() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("nested").join("admin.db");
        let lock = InstanceLock::acquire(&database).unwrap();
        assert!(lock.path().parent().unwrap().is_dir());
    }
}
