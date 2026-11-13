//! Workspace root confinement and path normalization.
//!
//! Every filesystem access made by the workspace crate goes through a
//! [`PathPolicy`]. Relative paths are rejected when they contain `..`, are
//! absolute (including Windows drive and UNC forms), or (for
//! [`PathPolicy::resolve_existing`]) resolve through a symlink to a location
//! outside the root.

use std::path::{Component, Path, PathBuf};

use crate::{Result, WorkspaceError};

/// Restricts all filesystem access to one canonical workspace root.
#[derive(Debug, Clone)]
pub struct PathPolicy {
    root: PathBuf,
    canonical_root: PathBuf,
}

impl PathPolicy {
    /// Create a policy for `root`.
    ///
    /// The root is canonicalized and must already exist and be a directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let canonical_root = std::fs::canonicalize(root).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                WorkspaceError::NotFound(root.to_string_lossy().into_owned())
            }
            _ => WorkspaceError::Io(error),
        })?;

        let metadata = std::fs::metadata(&canonical_root)?;
        if !metadata.is_dir() {
            return Err(WorkspaceError::InvalidRequest(format!(
                "workspace root is not a directory: {}",
                canonical_root.display()
            )));
        }

        Ok(Self {
            root: canonical_root.clone(),
            canonical_root,
        })
    }

    /// The canonical workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonical workspace root used for containment checks.
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    /// Normalize a workspace-relative path into `/`-separated components.
    ///
    /// An empty path (or `.`) normalizes to the root represented by `""`.
    /// Absolute paths, drive prefixes, UNC paths and any `..` component are
    /// rejected with [`WorkspaceError::PathDenied`].
    pub fn normalize_relative(rel: &str) -> Result<String> {
        if rel.is_empty() {
            return Ok(String::new());
        }
        if is_absolute_like(rel) {
            return Err(WorkspaceError::PathDenied(format!(
                "absolute paths are not allowed: {rel}"
            )));
        }

        let mut parts: Vec<String> = Vec::new();
        for component in Path::new(rel).components() {
            match component {
                Component::Normal(part) => {
                    parts.push(part.to_string_lossy().into_owned());
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(WorkspaceError::PathDenied(format!(
                        "parent directory segments are not allowed: {rel}"
                    )));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(WorkspaceError::PathDenied(format!(
                        "absolute paths are not allowed: {rel}"
                    )));
                }
            }
        }

        Ok(parts.join("/"))
    }

    /// Resolve a relative path to a lexical path inside the root.
    ///
    /// The result is joined and normalized but not canonicalized, so the path
    /// is not guaranteed to exist.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let normalized = Self::normalize_relative(rel)?;
        if normalized.is_empty() {
            Ok(self.root.clone())
        } else {
            Ok(self.root.join(Path::new(&normalized)))
        }
    }

    /// Resolve a relative path and verify its canonical target stays inside
    /// the workspace root.
    ///
    /// This is the only resolution method used for listing and reading, and it
    /// rejects symlink escapes.
    pub async fn resolve_existing(&self, rel: &str) -> Result<PathBuf> {
        let joined = self.resolve(rel)?;
        let canonical =
            tokio::fs::canonicalize(&joined)
                .await
                .map_err(|error| match error.kind() {
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                        WorkspaceError::NotFound(rel.to_string())
                    }
                    _ => WorkspaceError::Io(error),
                })?;

        if !canonical.starts_with(&self.canonical_root) {
            return Err(WorkspaceError::PathDenied(format!(
                "{rel} resolves outside the workspace root"
            )));
        }

        Ok(canonical)
    }
}

/// Reject absolute forms even on platforms where they are not `Path::is_absolute`.
fn is_absolute_like(rel: &str) -> bool {
    if rel.starts_with('/') || rel.starts_with('\\') {
        return true;
    }
    if Path::new(rel).is_absolute() {
        return true;
    }
    let bytes = rel.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn new_rejects_missing_and_non_directory_roots() {
        let dir = temp_root();
        assert!(PathPolicy::new(dir.path().join("missing")).is_err());

        let file = dir.path().join("file.txt");
        std::fs::write(&file, b"hello").expect("write");
        assert!(PathPolicy::new(&file).is_err());
    }

    #[test]
    fn resolve_allows_relative_paths() {
        let dir = temp_root();
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let resolved = policy.resolve("src/lib.rs").expect("resolve");
        assert_eq!(resolved, policy.root().join("src/lib.rs"));

        assert_eq!(policy.resolve("").expect("root"), policy.root());
        assert_eq!(
            policy.resolve("./src/./lib.rs").expect("normalized"),
            policy.root().join("src/lib.rs")
        );
    }

    #[test]
    fn resolve_denies_traversal_and_absolute_paths() {
        let dir = temp_root();
        let policy = PathPolicy::new(dir.path()).expect("policy");

        for denied in [
            "../../.ssh/id_rsa",
            "src/../../etc/passwd",
            "/etc/passwd",
            "C:\\Windows\\system32",
            "\\\\server\\share\\file",
            "\\rooted",
        ] {
            let error = policy.resolve(denied).expect_err(denied);
            assert!(
                matches!(error, WorkspaceError::PathDenied(_)),
                "{denied}: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn resolve_existing_canonicalizes_inside_root() {
        let dir = temp_root();
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/lib.rs"), b"fn main() {}").expect("write");

        let policy = PathPolicy::new(dir.path()).expect("policy");
        let resolved = policy
            .resolve_existing("src/lib.rs")
            .await
            .expect("resolve");
        assert_eq!(
            resolved,
            std::fs::canonicalize(dir.path().join("src/lib.rs")).expect("canonical")
        );
    }

    #[tokio::test]
    async fn resolve_existing_rejects_missing_paths() {
        let dir = temp_root();
        let policy = PathPolicy::new(dir.path()).expect("policy");
        let error = policy
            .resolve_existing("does-not-exist")
            .await
            .expect_err("missing");
        assert!(matches!(error, WorkspaceError::NotFound(_)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_existing_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = temp_root();
        let outside = temp_root();
        std::fs::write(outside.path().join("secret.txt"), b"top secret").expect("write");
        symlink(outside.path(), root.path().join("escape")).expect("symlink");

        let policy = PathPolicy::new(root.path()).expect("policy");
        let error = policy
            .resolve_existing("escape/secret.txt")
            .await
            .expect_err("escape denied");
        assert!(matches!(error, WorkspaceError::PathDenied(_)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_existing_allows_symlink_inside_root() {
        use std::os::unix::fs::symlink;

        let root = temp_root();
        std::fs::create_dir_all(root.path().join("real")).expect("mkdir");
        std::fs::write(root.path().join("real/lib.rs"), b"fn main() {}").expect("write");
        symlink(root.path().join("real"), root.path().join("link")).expect("symlink");

        let policy = PathPolicy::new(root.path()).expect("policy");
        let resolved = policy
            .resolve_existing("link/lib.rs")
            .await
            .expect("inside link allowed");
        assert_eq!(
            resolved,
            std::fs::canonicalize(root.path().join("real/lib.rs")).expect("canonical")
        );
    }
}
