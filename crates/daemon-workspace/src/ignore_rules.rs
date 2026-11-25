//! Ignore-rule evaluation shared by directory listing and search.
//!
//! Two independent sources are supported:
//!
//! * default ignored names supplied through [`WorkspaceSettings`] (always
//!   applied), and
//! * `.gitignore` / `.ignore` rules read from the workspace root and from the
//!   directories between the root and the directory being listed or searched.
//!
//! Listings and search share [`IgnoreScope`] so a path hidden by a nested
//! ignore file is hidden consistently in both.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::settings::WorkspaceSettings;

/// Compiled ignore rules for one workspace root.
pub(crate) struct IgnoreRules {
    defaults: HashSet<String>,
    gitignore: Gitignore,
}

/// Cache entry keyed by workspace root.
struct CachedRules {
    /// Modification time and size of each ignore file when compiled.
    stamp: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    /// Sorted default-ignore names that contributed to the compiled rules.
    defaults: Vec<String>,
    rules: Arc<IgnoreRules>,
}

/// Compiled rules are pure functions of the ignore files, so listings and
/// watcher flushes can share them instead of reparsing per directory.
static RULES_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedRules>>> = OnceLock::new();

/// Keep the cache bounded if workspaces are registered and removed over time.
const RULES_CACHE_LIMIT: usize = 256;

/// The ignore files at and above one directory, from shallowest to deepest.
fn ignore_files_for(root: &Path, directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(relative) = directory.strip_prefix(root) else {
        return files;
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        for name in [".gitignore", ".ignore"] {
            let path = current.join(name);
            if path.is_file() {
                files.push(path);
            }
        }
    }
    files
}

fn ignore_file_stamp(files: &[PathBuf]) -> Vec<(PathBuf, Option<(SystemTime, u64)>)> {
    files
        .iter()
        .map(|path| {
            let stamp = std::fs::metadata(path)
                .ok()
                .and_then(|metadata| metadata.modified().ok().map(|time| (time, metadata.len())));
            (path.clone(), stamp)
        })
        .collect()
}

fn root_ignore_stamp(root: &Path) -> Vec<(PathBuf, Option<(SystemTime, u64)>)> {
    ignore_file_stamp(
        &[".gitignore", ".ignore"]
            .iter()
            .map(|name| root.join(name))
            .collect::<Vec<_>>(),
    )
}

impl IgnoreRules {
    /// Load `.gitignore` and `.ignore` from the workspace root.
    ///
    /// Malformed rules are logged and ignored so a broken ignore file cannot
    /// make the workspace unreadable. Results are cached until either ignore
    /// file changes.
    pub(crate) fn load(root: &Path, settings: &WorkspaceSettings) -> Arc<Self> {
        let root = root.to_path_buf();
        let stamp = root_ignore_stamp(&root);
        let mut defaults: Vec<String> = settings.default_ignores.to_vec();
        defaults.sort_unstable();

        let cache = RULES_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        {
            let cached = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(entry) = cached.get(&root) {
                if entry.stamp == stamp && entry.defaults == defaults {
                    return Arc::clone(&entry.rules);
                }
            }
        }

        let rules = Arc::new(Self::compile(&root, settings));
        let mut cached = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cached.len() >= RULES_CACHE_LIMIT {
            cached.clear();
        }
        cached.insert(
            root,
            CachedRules {
                stamp,
                defaults,
                rules: Arc::clone(&rules),
            },
        );
        rules
    }

    fn compile(root: &Path, settings: &WorkspaceSettings) -> Self {
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
}

/// Ignore rules that apply inside one directory.
///
/// Root defaults and root ignore files come from the cached [`IgnoreRules`];
/// per-directory ignore files between the root and the directory add
/// matchers. Directory listings and search both use this scope, so nested
/// rules hide the same paths in both.
pub(crate) struct IgnoreScope {
    root: PathBuf,
    /// The directory this scope was loaded for.
    directory: PathBuf,
    base: Arc<IgnoreRules>,
    /// Matchers from shallower to deeper, each paired with its base directory.
    /// Only directories between the root and `directory` inclusive have
    /// matchers here; the root's files live in `base`.
    nested: Vec<(PathBuf, Gitignore)>,
    /// Whether `directory` itself is ignored. Children cannot be re-included
    /// below an excluded directory (Git has the same rule), so this is
    /// checked once instead of per entry.
    directory_ignored: bool,
}

struct CachedScope {
    stamp: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    defaults: Vec<String>,
    scope: Arc<IgnoreScope>,
}

/// Nested scopes are immutable for a given set of ignore-file stamps, like
/// the root rules cache.
static SCOPE_CACHE: OnceLock<Mutex<HashMap<(PathBuf, PathBuf), CachedScope>>> = OnceLock::new();

/// Listings visit many directories; keep the scope cache larger than the root
/// cache and clear it wholesale when full.
const SCOPE_CACHE_LIMIT: usize = 512;

impl IgnoreScope {
    /// Load the rules that apply to `directory`, which must be inside `root`.
    ///
    /// Malformed ignore files are logged and skipped.
    pub(crate) fn load(root: &Path, directory: &Path, settings: &WorkspaceSettings) -> Arc<Self> {
        let root = root.to_path_buf();
        let directory = directory.to_path_buf();
        let files = ignore_files_for(&root, &directory);
        // Root files feed the base matcher, so they belong to this cache key
        // as well as to the root-rules cache.
        let mut stamp = root_ignore_stamp(&root);
        stamp.extend(ignore_file_stamp(&files));
        let mut defaults: Vec<String> = settings.default_ignores.to_vec();
        defaults.sort_unstable();

        let cache = SCOPE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let key = (root.clone(), directory.clone());
        {
            let cached = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(entry) = cached.get(&key) {
                if entry.stamp == stamp && entry.defaults == defaults {
                    return Arc::clone(&entry.scope);
                }
            }
        }

        let mut nested: Vec<(PathBuf, Gitignore)> = Vec::new();
        for file in &files {
            let Some(parent) = file.parent() else {
                continue;
            };
            if nested
                .last()
                .is_some_and(|(existing, _)| existing == parent)
            {
                continue;
            }
            let mut builder = GitignoreBuilder::new(parent);
            for name in [".gitignore", ".ignore"] {
                let path = parent.join(name);
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
            match builder.build() {
                Ok(matcher) => nested.push((parent.to_path_buf(), matcher)),
                Err(error) => tracing::warn!(
                    path = %parent.display(),
                    error = %error,
                    "failed to compile nested ignore rules"
                ),
            }
        }

        let mut scope = Self {
            base: IgnoreRules::load(&root, settings),
            directory_ignored: false,
            root,
            directory,
            nested,
        };
        scope.directory_ignored = scope.target_is_ignored();
        let scope = Arc::new(scope);
        let mut cached = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cached.len() >= SCOPE_CACHE_LIMIT {
            cached.clear();
        }
        cached.insert(
            key,
            CachedScope {
                stamp,
                defaults,
                scope: Arc::clone(&scope),
            },
        );
        scope
    }

    /// Return `true` when `abs_path` is hidden by defaults or by an ignore
    /// file at or above its directory.
    ///
    /// Rules are evaluated from the workspace root downwards so a deeper
    /// ignore file decides for itself, matching Git's precedence: a nested
    /// `!pattern` may re-include a path ignored at the root, but never a path
    /// inside an excluded directory.
    pub(crate) fn is_ignored(&self, abs_path: &Path, is_dir: bool) -> bool {
        if let Some(name) = abs_path.file_name().and_then(|name| name.to_str()) {
            if self.base.defaults.contains(name) {
                return true;
            }
        }
        if self.directory_ignored {
            return true;
        }

        let Ok(relative) = abs_path.strip_prefix(&self.directory) else {
            return false;
        };
        let mut current = self.directory.clone();
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let is_last = components.peek().is_none();
            current.push(component);
            match self.decide(&current, !is_last || is_dir) {
                Some(true) => return true,
                Some(false) if is_last => return false,
                // A re-included directory continues the walk; its children
                // still need their own rules.
                _ => {}
            }
        }
        false
    }

    /// Whether the scope's own directory is ignored by its ancestors.
    fn target_is_ignored(&self) -> bool {
        let mut current = self.root.clone();
        let Ok(relative) = self.directory.strip_prefix(&self.root) else {
            return false;
        };
        for component in relative.components() {
            current.push(component);
            if self.decide(&current, true) == Some(true) {
                return true;
            }
        }
        false
    }

    /// Deepest ignore file's verdict for one path: `Some(true)` ignored,
    /// `Some(false)` whitelisted, `None` no rule matched.
    ///
    /// A directory's own ignore file never applies to that directory.
    fn decide(&self, abs_path: &Path, is_dir: bool) -> Option<bool> {
        for (directory, matcher) in self.nested.iter().rev() {
            if abs_path.starts_with(directory) && abs_path != directory {
                match matcher.matched(abs_path, is_dir) {
                    ignore::Match::None => continue,
                    ignore::Match::Ignore(_) => return Some(true),
                    ignore::Match::Whitelist(_) => return Some(false),
                }
            }
        }
        match self.base.gitignore.matched(abs_path, is_dir) {
            ignore::Match::None => None,
            ignore::Match::Ignore(_) => Some(true),
            ignore::Match::Whitelist(_) => Some(false),
        }
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
        let rules = IgnoreScope::load(dir.path(), dir.path(), &settings);

        assert!(rules.is_ignored(&dir.path().join("node_modules"), true));
        assert!(rules.is_ignored(&dir.path().join("ignored.txt"), false));
        assert!(rules.is_ignored(&dir.path().join("build"), true));
        // A child of an ignored directory is ignored through its parent.
        assert!(rules.is_ignored(&dir.path().join("build/out.bin"), false));
        assert!(!rules.is_ignored(&dir.path().join("src/main.rs"), false));
    }

    #[test]
    fn nested_rules_override_root_rules_but_not_excluded_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A nested ignore file may re-include a path the root ignored...
        std::fs::write(dir.path().join(".gitignore"), "*.log\nsecret/\n").expect("write");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/.gitignore"), "!keep.log\n").expect("write");
        std::fs::write(dir.path().join("src/keep.log"), b"x").expect("write");
        std::fs::write(dir.path().join("src/other.log"), b"x").expect("write");
        // ...but not a path inside an excluded directory.
        std::fs::create_dir_all(dir.path().join("secret/inner")).expect("mkdir");
        std::fs::write(dir.path().join("secret/inner/.gitignore"), "!*.log\n").expect("write");
        std::fs::write(dir.path().join("secret/inner/keep.log"), b"x").expect("write");

        let settings = WorkspaceSettings::default();
        let scope = IgnoreScope::load(dir.path(), &dir.path().join("src"), &settings);
        assert!(!scope.is_ignored(&dir.path().join("src/keep.log"), false));
        assert!(scope.is_ignored(&dir.path().join("src/other.log"), false));

        let secret_scope =
            IgnoreScope::load(dir.path(), &dir.path().join("secret/inner"), &settings);
        assert!(secret_scope.directory_ignored);
        assert!(secret_scope.is_ignored(&dir.path().join("secret/inner/keep.log"), false));
    }

    #[test]
    fn scope_applies_root_and_nested_rules_inside_a_subdirectory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".gitignore"), "root-ignored.txt\n").expect("write");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/.gitignore"), "nested-ignored.txt\n").expect("write");
        std::fs::write(dir.path().join("src/root-ignored.txt"), b"x").expect("write");
        std::fs::write(dir.path().join("src/nested-ignored.txt"), b"x").expect("write");
        std::fs::write(dir.path().join("src/kept.txt"), b"x").expect("write");

        let settings = WorkspaceSettings::default();
        let scope = IgnoreScope::load(dir.path(), &dir.path().join("src"), &settings);
        assert!(scope.is_ignored(&dir.path().join("src/root-ignored.txt"), false));
        assert!(scope.is_ignored(&dir.path().join("src/nested-ignored.txt"), false));
        assert!(!scope.is_ignored(&dir.path().join("src/kept.txt"), false));
    }

    #[test]
    fn cache_invalidates_when_ignore_files_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".gitignore"), "a.txt\n").expect("write");
        let settings = WorkspaceSettings::default();
        let rules = IgnoreScope::load(dir.path(), dir.path(), &settings);
        assert!(rules.is_ignored(&dir.path().join("a.txt"), false));

        // The new content differs in size, so the stamp changes even on
        // filesystems with coarse modification timestamps.
        std::fs::write(dir.path().join(".gitignore"), "b-longer.txt\n").expect("write");
        let refreshed = IgnoreScope::load(dir.path(), dir.path(), &settings);
        assert!(!refreshed.is_ignored(&dir.path().join("a.txt"), false));
        assert!(refreshed.is_ignored(&dir.path().join("b-longer.txt"), false));
    }
}
