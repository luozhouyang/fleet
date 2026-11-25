//! Validation and normalization of user-supplied relative paths.
//!
//! Workspace file access and Git pathspecs share the same lexical rules:
//! reject absolute forms (including Windows drive/UNC spellings), `..`
//! traversal and NUL bytes, and normalize the result to `/`-separated
//! components. Git callers additionally reject option-like and pathspec-magic
//! prefixes and `.git` components, because those values are passed to the
//! `git` command line.

use std::fmt;
use std::path::{Component, Path};

/// Why a user-supplied path was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRejection {
    /// The path is empty or whitespace-only where a real path is required.
    Empty,
    /// The path contains a NUL byte.
    NulByte,
    /// The path is absolute, including Windows drive and UNC forms.
    Absolute,
    /// The path contains a `..` component.
    ParentTraversal,
    /// The path starts with `-` and would be parsed as an option.
    OptionLike,
    /// The path starts with `:` and would be parsed as pathspec magic.
    PathspecMagic,
    /// The path contains a `.git` component.
    GitInternals,
}

impl fmt::Display for PathRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "empty path",
            Self::NulByte => "NUL byte",
            Self::Absolute => "absolute paths are not allowed",
            Self::ParentTraversal => "`..` is not allowed",
            Self::OptionLike => "must not start with `-`",
            Self::PathspecMagic => "pathspec magic is not allowed",
            Self::GitInternals => "the `.git` directory is not accessible",
        })
    }
}

/// Per-call-site rules layered on top of the shared lexical checks.
#[derive(Debug, Clone, Copy)]
pub struct PathRules {
    /// Reject empty or whitespace-only input instead of normalizing it to `""`.
    pub require_non_empty: bool,
    /// Reject a leading `-` (Git option injection).
    pub reject_option_like: bool,
    /// Reject a leading `:` (Git pathspec magic).
    pub reject_pathspec_magic: bool,
    /// Reject any component named `.git`.
    pub reject_git_internals: bool,
}

impl PathRules {
    /// Rules for workspace-relative file access.
    ///
    /// An empty path denotes the workspace root and is allowed.
    pub const WORKSPACE_RELATIVE: Self = Self {
        require_non_empty: false,
        reject_option_like: false,
        reject_pathspec_magic: false,
        reject_git_internals: false,
    };

    /// Rules for paths passed to Git as a pathspec or a `--` argument.
    pub const GIT_PATHSPEC: Self = Self {
        require_non_empty: true,
        reject_option_like: true,
        reject_pathspec_magic: true,
        reject_git_internals: true,
    };
}

/// Validate `input` and return it normalized to `/`-separated components.
///
/// `CurDir` components are dropped, so `./src/./lib.rs` becomes `src/lib.rs`.
pub fn normalize_relative(input: &str, rules: PathRules) -> Result<String, PathRejection> {
    if rules.require_non_empty && input.trim().is_empty() {
        return Err(PathRejection::Empty);
    }
    if input.bytes().any(|byte| byte == 0) {
        return Err(PathRejection::NulByte);
    }
    if rules.reject_option_like && input.starts_with('-') {
        return Err(PathRejection::OptionLike);
    }
    if rules.reject_pathspec_magic && input.starts_with(':') {
        return Err(PathRejection::PathspecMagic);
    }
    if is_absolute_like(input) {
        return Err(PathRejection::Absolute);
    }

    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(input).components() {
        match component {
            Component::Normal(part) => {
                if rules.reject_git_internals && part == ".git" {
                    return Err(PathRejection::GitInternals);
                }
                parts.push(part.to_string_lossy().into_owned());
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(PathRejection::ParentTraversal),
            Component::RootDir | Component::Prefix(_) => {
                return Err(PathRejection::Absolute);
            }
        }
    }
    Ok(parts.join("/"))
}

/// Reject absolute forms even on platforms where they are not `Path::is_absolute`.
fn is_absolute_like(input: &str) -> bool {
    if input.starts_with('/') || input.starts_with('\\') {
        return true;
    }
    if Path::new(input).is_absolute() {
        return true;
    }
    let bytes = input.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_paths_normalize_and_allow_the_root() {
        assert_eq!(
            normalize_relative("", PathRules::WORKSPACE_RELATIVE).expect("root"),
            ""
        );
        assert_eq!(
            normalize_relative("./src/./lib.rs", PathRules::WORKSPACE_RELATIVE).expect("path"),
            "src/lib.rs"
        );
        assert_eq!(
            normalize_relative("dir with space/file", PathRules::WORKSPACE_RELATIVE).expect("path"),
            "dir with space/file"
        );
    }

    #[test]
    fn shared_lexical_rules_apply_in_every_mode() {
        let cases = [
            ("../etc/passwd", PathRejection::ParentTraversal),
            ("src/../../etc/passwd", PathRejection::ParentTraversal),
            ("/etc/passwd", PathRejection::Absolute),
            ("\\\\server\\share\\file", PathRejection::Absolute),
            ("\\rooted", PathRejection::Absolute),
            ("C:\\Windows\\system32", PathRejection::Absolute),
            ("a\0b", PathRejection::NulByte),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_relative(input, PathRules::WORKSPACE_RELATIVE),
                Err(expected),
                "{input:?}"
            );
        }
    }

    #[test]
    fn git_rules_reject_options_magic_and_internals() {
        assert_eq!(
            normalize_relative("", PathRules::GIT_PATHSPEC),
            Err(PathRejection::Empty)
        );
        assert_eq!(
            normalize_relative("   ", PathRules::GIT_PATHSPEC),
            Err(PathRejection::Empty)
        );
        assert_eq!(
            normalize_relative("-rf", PathRules::GIT_PATHSPEC),
            Err(PathRejection::OptionLike)
        );
        assert_eq!(
            normalize_relative(":path", PathRules::GIT_PATHSPEC),
            Err(PathRejection::PathspecMagic)
        );
        assert_eq!(
            normalize_relative(".git/config", PathRules::GIT_PATHSPEC),
            Err(PathRejection::GitInternals)
        );
        assert_eq!(
            normalize_relative("sub/.git", PathRules::GIT_PATHSPEC),
            Err(PathRejection::GitInternals)
        );
        assert_eq!(
            normalize_relative("./src/lib.rs", PathRules::GIT_PATHSPEC).expect("valid"),
            "src/lib.rs"
        );
        // `.` is a valid revision-relative pathspec even though it normalizes
        // to the empty string.
        assert_eq!(
            normalize_relative(".", PathRules::GIT_PATHSPEC).expect("valid"),
            ""
        );
    }

    #[test]
    fn workspace_mode_keeps_git_specific_paths_valid() {
        for input in [".git/config", "-rf", ":path"] {
            assert!(
                normalize_relative(input, PathRules::WORKSPACE_RELATIVE).is_ok(),
                "{input:?}"
            );
        }
    }
}
