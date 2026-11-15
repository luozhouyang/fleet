//! Directory listing, sorting, pagination and index refresh.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fleet_core_protocol::workspace::{DirectoryListing, EntryType, WorkspaceEntry};
use fleet_daemon_storage::DaemonStorage;
use serde::{Deserialize, Serialize};

use crate::ignore_rules::IgnoreRules;
use crate::path_policy::PathPolicy;
use crate::settings::WorkspaceSettings;
use crate::{Result, WorkspaceError};

/// Default page size for directory listings.
pub(crate) const DEFAULT_LIMIT: usize = 200;

/// Hard upper bound for directory listing pages.
pub(crate) const MAX_LIMIT: usize = 1000;

/// Read, filter and sort the entries of one directory.
///
/// The result includes every visible entry (not just one page) so callers can
/// refresh the SQLite index with the complete parent listing.
pub(crate) async fn read_directory_entries(
    policy: &PathPolicy,
    settings: &WorkspaceSettings,
    parent: &str,
    show_ignored: bool,
) -> Result<Vec<WorkspaceEntry>> {
    let resolved = policy.resolve_existing(parent).await?;
    let metadata = tokio::fs::metadata(&resolved)
        .await
        .map_err(map_missing(parent))?;
    if !metadata.is_dir() {
        return Err(WorkspaceError::InvalidRequest(format!(
            "not a directory: {parent}"
        )));
    }

    let rules = IgnoreRules::load(policy.root(), settings);
    let mut dir = tokio::fs::read_dir(&resolved)
        .await
        .map_err(map_missing(parent))?;

    let mut entries = Vec::new();
    while let Some(dir_entry) = dir.next_entry().await? {
        let name = dir_entry.file_name().to_string_lossy().into_owned();
        let abs_path = dir_entry.path();

        // Use symlink metadata so we neither follow links nor fail on broken
        // links when building the listing.
        let link_metadata = match tokio::fs::symlink_metadata(&abs_path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(WorkspaceError::Io(error)),
        };

        let file_type = link_metadata.file_type();
        let is_symlink = file_type.is_symlink();
        let is_dir = !is_symlink && file_type.is_dir();

        let ignored = rules.is_ignored(&abs_path, is_dir);
        if ignored && !show_ignored {
            continue;
        }

        let entry_type = if is_symlink {
            EntryType::Symlink
        } else if is_dir {
            EntryType::Directory
        } else {
            EntryType::File
        };

        let size = if matches!(entry_type, EntryType::File) {
            Some(link_metadata.len())
        } else {
            None
        };
        let modified_at = link_metadata
            .modified()
            .ok()
            .map(crate::system_time_to_timestamp);

        entries.push(WorkspaceEntry {
            path: join_relative(parent, &name),
            name,
            entry_type,
            size,
            modified_at,
            git_status: None,
            ignored,
        });
    }

    sort_entries(&mut entries);
    Ok(entries)
}

/// Sort directories first, then files, by case-insensitive name.
pub(crate) fn sort_entries(entries: &mut [WorkspaceEntry]) {
    entries.sort_by(|left, right| {
        let left_dir = left.entry_type == EntryType::Directory;
        let right_dir = right.entry_type == EntryType::Directory;
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
}

/// Split a sorted entry list into one page and the next opaque cursor.
pub(crate) fn paginate(
    entries: Vec<WorkspaceEntry>,
    cursor: Option<&str>,
    limit: usize,
) -> (Vec<WorkspaceEntry>, Option<String>) {
    let limit = limit.clamp(1, MAX_LIMIT);
    let cursor = cursor.and_then(decode_cursor);

    let start = match cursor {
        None => 0,
        Some(cursor) => match entries.iter().position(|entry| entry.path == cursor.path) {
            Some(index) => index + 1,
            None => entries
                .iter()
                .position(|entry| sort_key(entry) > cursor.sort_key())
                .unwrap_or(entries.len()),
        },
    };

    if start >= entries.len() {
        return (Vec::new(), None);
    }

    let end = (start + limit).min(entries.len());
    let mut page = Vec::with_capacity(end - start);
    page.extend_from_slice(&entries[start..end]);

    let next_cursor = if end < entries.len() {
        page.last().map(encode_cursor)
    } else {
        None
    };

    (page, next_cursor)
}

/// Replace the cached entries for one parent directory.
///
/// Index errors never fail a listing; they are logged at warn level.
pub(crate) async fn refresh_index(
    storage: &DaemonStorage,
    workspace_id: &str,
    parent: &str,
    entries: &[WorkspaceEntry],
) {
    if let Err(error) = storage.entries.replace(workspace_id, parent, entries).await {
        tracing::warn!(
            workspace_id,
            parent,
            error = %error,
            "failed to refresh workspace directory index"
        );
    }
}

/// Build a [`DirectoryListing`] from filesystem entries and refresh the index.
pub(crate) async fn list_directory(
    storage: &DaemonStorage,
    policy: &PathPolicy,
    settings: &WorkspaceSettings,
    workspace_id: &str,
    parent: &str,
    cursor: Option<&str>,
    limit: usize,
    show_ignored: bool,
) -> Result<DirectoryListing> {
    match read_directory_entries(policy, settings, parent, show_ignored).await {
        Ok(all) => {
            refresh_index(storage, workspace_id, parent, &all).await;
            let (entries, next_cursor) = paginate(all, cursor, limit);
            Ok(DirectoryListing {
                path: parent.to_string(),
                entries,
                next_cursor,
            })
        }
        Err(WorkspaceError::NotFound(message)) => Err(WorkspaceError::NotFound(message)),
        Err(WorkspaceError::Io(error)) => {
            // A transient filesystem failure may still be served from the
            // cached index. Missing paths are handled above by the reader.
            let mut cached = storage.entries.list(workspace_id, parent).await?;
            if cached.is_empty() {
                return Err(WorkspaceError::Io(error));
            }
            sort_entries(&mut cached);
            let (entries, next_cursor) = paginate(cached, cursor, limit);
            Ok(DirectoryListing {
                path: parent.to_string(),
                entries,
                next_cursor,
            })
        }
        Err(error) => Err(error),
    }
}

fn map_missing(parent: &str) -> impl FnOnce(std::io::Error) -> WorkspaceError + '_ {
    move |error| match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
            WorkspaceError::NotFound(parent.to_string())
        }
        _ => WorkspaceError::Io(error),
    }
}

fn join_relative(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// Encode a cursor so callers cannot rely on its contents.
/// Sort key used by [`sort_entries`] and pagination.
fn sort_key(entry: &WorkspaceEntry) -> (bool, String, String) {
    (
        entry.entry_type != EntryType::Directory,
        entry.name.to_lowercase(),
        entry.name.clone(),
    )
}

#[derive(Debug, Serialize, Deserialize)]
struct Cursor {
    path: String,
    name: String,
    is_dir: bool,
}

impl Cursor {
    fn sort_key(&self) -> (bool, String, String) {
        (!self.is_dir, self.name.to_lowercase(), self.name.clone())
    }
}

fn encode_cursor(entry: &WorkspaceEntry) -> String {
    let cursor = Cursor {
        path: entry.path.clone(),
        name: entry.name.clone(),
        is_dir: entry.entry_type == EntryType::Directory,
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).unwrap_or_default())
}

/// Decode a cursor, tolerating callers that pass a raw path.
fn decode_cursor(cursor: &str) -> Option<Cursor> {
    let bytes = match URL_SAFE_NO_PAD.decode(cursor.as_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => {
            return Some(Cursor {
                path: cursor.to_string(),
                name: cursor.to_string(),
                is_dir: false,
            })
        }
    };
    if let Ok(value) = serde_json::from_slice::<Cursor>(&bytes) {
        return Some(value);
    }
    String::from_utf8(bytes).ok().map(|path| Cursor {
        name: path.clone(),
        path,
        is_dir: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn setup() -> (
        tempfile::TempDir,
        Arc<DaemonStorage>,
        PathPolicy,
        WorkspaceSettings,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let database = Arc::new(DaemonStorage::connect_in_memory().await.expect("db"));
        let policy = PathPolicy::new(dir.path()).expect("policy");
        (dir, database, policy, WorkspaceSettings::default())
    }

    #[tokio::test]
    async fn lists_sorted_directories_first() {
        let (dir, database, policy, settings) = setup().await;
        std::fs::write(dir.path().join("zeta.txt"), b"z").expect("write");
        std::fs::write(dir.path().join("Alpha.txt"), b"a").expect("write");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");

        let listing = list_directory(
            &database, &policy, &settings, "ws_test", "", None, 200, false,
        )
        .await
        .expect("listing");

        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["src", "Alpha.txt", "zeta.txt"]);
        assert_eq!(listing.entries[0].entry_type, EntryType::Directory);
        assert!(listing.entries[1].size.is_some());
        assert!(listing.entries[1].modified_at.is_some());
    }

    #[tokio::test]
    async fn hides_default_ignores_and_gitignore_rules() {
        let (dir, database, policy, settings) = setup().await;
        std::fs::write(dir.path().join(".gitignore"), "secret.txt\n").expect("write");
        std::fs::write(dir.path().join("visible.txt"), b"v").expect("write");
        std::fs::write(dir.path().join("secret.txt"), b"s").expect("write");
        std::fs::create_dir_all(dir.path().join("node_modules")).expect("mkdir");

        let listing = list_directory(
            &database, &policy, &settings, "ws_test", "", None, 200, false,
        )
        .await
        .expect("listing");
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec![".gitignore", "visible.txt"]);

        let listing = list_directory(
            &database, &policy, &settings, "ws_test", "", None, 200, true,
        )
        .await
        .expect("listing");
        let ignored: Vec<&str> = listing
            .entries
            .iter()
            .filter(|entry| entry.ignored)
            .map(|entry| entry.name.as_str())
            .collect();
        assert!(ignored.contains(&"node_modules"));
        assert!(ignored.contains(&"secret.txt"));
        assert!(!ignored.contains(&"visible.txt"));
    }

    #[tokio::test]
    async fn pagination_returns_no_duplicates() {
        let (dir, database, policy, settings) = setup().await;
        for index in 0..5 {
            std::fs::write(dir.path().join(format!("file{index}.txt")), b"x").expect("write");
        }

        let mut cursor: Option<String> = None;
        let mut seen = Vec::new();
        loop {
            let listing = list_directory(
                &database,
                &policy,
                &settings,
                "ws_test",
                "",
                cursor.as_deref(),
                2,
                false,
            )
            .await
            .expect("listing");
            assert!(listing.entries.len() <= 2);
            seen.extend(listing.entries.iter().map(|entry| entry.path.clone()));
            match listing.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }

        assert_eq!(seen.len(), 5);
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 5);
    }

    #[tokio::test]
    async fn listing_populates_sqlite_index() {
        let (dir, database, policy, settings) = setup().await;
        std::fs::write(dir.path().join("a.txt"), b"a").expect("write");

        list_directory(
            &database, &policy, &settings, "ws_test", "", None, 200, false,
        )
        .await
        .expect("listing");

        let cached = database
            .entries
            .list("ws_test", "")
            .await
            .expect("cached entries");
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].path, "a.txt");
    }

    #[tokio::test]
    async fn lists_nested_directory() {
        let (dir, database, policy, settings) = setup().await;
        std::fs::create_dir_all(dir.path().join("src/inner")).expect("mkdir");
        std::fs::write(dir.path().join("src/lib.rs"), b"x").expect("write");

        let listing = list_directory(
            &database, &policy, &settings, "ws_test", "src", None, 200, false,
        )
        .await
        .expect("listing");
        assert_eq!(listing.path, "src");
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["inner", "lib.rs"]);
        assert_eq!(listing.entries[1].path, "src/lib.rs");
    }
}
