//! Ignore-rule evaluation shared by directory listing and search.
//!
//! Two independent sources are supported:
//!
//! * default ignored names supplied through [`WorkspaceSettings`] (always
//!   applied), and
//! * `.gitignore` / `.ignore` rules read from the workspace root.

use std::collections::HashSet;
use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::settings::WorkspaceSettings;

/// Compiled ignore rules for one workspace root.
pub(crate) struct IgnoreRules {
    defaults: HashSet<String>,
    gitignore: Gitignore,
}

impl IgnoreRules {
    /// Load `.gitignore` and `.ignore` from the workspace root.
    ///
    /// Malformed rules are logged and ignored so a broken ignore file cannot
    /// make the workspace unreadable.
    pub(crate) fn load(root: &Path, settings: &WorkspaceSettings) -> Self {
        let mut builder = GitignoreBuilder::new(root);
        for file_name in [".gitignore", ".ignore"] {
            let path = root.join(file_name);
            if path.is_file() {
                if let Some(error) = builder.add(&path) {
                    tracing::warn!(
                        path = %path.display(),
                        error = %error,
                        "ignoring malformed ignore file"
                    );
                }
            }
        }

        let gitignore = builder.build().unwrap_or_else(|error| {
            tracing::warn!(error = %error, "failed to compile workspace ignore rules");
            Gitignore::empty()
        });

        Self {
            defaults: settings.default_ignores.iter().cloned().collect(),
            gitignore,
        }
    }

    /// Return `true` when `abs_path` is ignored by default names or by the
    /// root ignore files.
    pub(crate) fn is_ignored(&self, abs_path: &Path, is_dir: bool) -> bool {
        if let Some(name) = abs_path.file_name().and_then(|name| name.to_str()) {
            if self.defaults.contains(name) {
                return true;
            }
        }

        self.gitignore
            .matched_path_or_any_parents(abs_path, is_dir)
            .is_ignore()
    }

    /// Return `true` when any component of a workspace-relative path matches a
    /// default ignored name.
    pub(crate) fn has_default_component(&self, relative_path: &Path) -> bool {
        relative_path.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| self.defaults.contains(name))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_gitignore_are_applied() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".gitignore"), "ignored.txt\nbuild/\n").expect("write");
        std::fs::create_dir_all(dir.path().join("build")).expect("mkdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("ignored.txt"), b"x").expect("write");
        std::fs::write(dir.path().join("src/main.rs"), b"x").expect("write");

        let settings = WorkspaceSettings::default();
        let rules = IgnoreRules::load(dir.path(), &settings);

        assert!(rules.is_ignored(&dir.path().join("node_modules"), true));
        assert!(rules.is_ignored(&dir.path().join("ignored.txt"), false));
        assert!(rules.is_ignored(&dir.path().join("build"), true));
        // A child of an ignored directory is ignored through its parent.
        assert!(rules.is_ignored(&dir.path().join("build/out.bin"), false));
        assert!(!rules.is_ignored(&dir.path().join("src/main.rs"), false));
    }
}
