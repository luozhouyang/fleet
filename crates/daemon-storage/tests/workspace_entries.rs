mod common;

use common::*;
use fleet_core_protocol::git::GitFileStatus;
use fleet_core_protocol::workspace::EntryType;
use fleet_daemon_storage::DaemonStorage;

#[tokio::test]
async fn replace_list_and_delete_entries() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let ws = "ws_1";
    let other = "ws_2";

    db.entries
        .replace(
            ws,
            "src",
            &[
                entry("src/b.rs", "b.rs", EntryType::File),
                entry("src/a.rs", "a.rs", EntryType::File),
            ],
        )
        .await
        .unwrap();
    db.entries
        .replace(
            ws,
            "docs",
            &[entry("docs/readme.md", "readme.md", EntryType::File)],
        )
        .await
        .unwrap();
    db.entries
        .replace(
            ws,
            "",
            &[
                entry("root.txt", "root.txt", EntryType::File),
                entry("src_extra", "src_extra", EntryType::Directory),
            ],
        )
        .await
        .unwrap();
    db.entries
        .replace(
            other,
            "src",
            &[entry("src/other.rs", "other.rs", EntryType::File)],
        )
        .await
        .unwrap();

    let src = db.entries.list(ws, "src").await.unwrap();
    assert_eq!(paths(&src), vec!["src/a.rs", "src/b.rs"]);
    assert_eq!(src[0].name, "a.rs");
    assert_eq!(src[0].entry_type, EntryType::File);
    assert_eq!(src[0].size, Some(42));
    assert_eq!(src[0].modified_at, Some(ts(1_700_000_123)));
    assert_eq!(src[0].git_status, Some(GitFileStatus::Modified));
    assert!(src[0].ignored);

    // Replacing one parent leaves its siblings and other parents untouched.
    db.entries
        .replace(ws, "src", &[entry("src/c.rs", "c.rs", EntryType::File)])
        .await
        .unwrap();
    assert_eq!(
        paths(&db.entries.list(ws, "src").await.unwrap()),
        vec!["src/c.rs"]
    );
    assert_eq!(db.entries.list(ws, "docs").await.unwrap().len(), 1);
    assert_eq!(db.entries.list(ws, "").await.unwrap().len(), 2);

    // A nested parent is removed by deleting its ancestor subtree.
    db.entries
        .replace(
            ws,
            "src/nested",
            &[entry("src/nested/x.rs", "x.rs", EntryType::File)],
        )
        .await
        .unwrap();

    db.entries.delete_under(ws, "src").await.unwrap();
    assert!(db.entries.list(ws, "src").await.unwrap().is_empty());
    assert!(db.entries.list(ws, "src/nested").await.unwrap().is_empty());
    assert_eq!(db.entries.list(ws, "docs").await.unwrap().len(), 1);
    assert_eq!(
        paths(&db.entries.list(ws, "").await.unwrap()),
        vec!["root.txt", "src_extra"]
    );
    assert_eq!(db.entries.list(other, "src").await.unwrap().len(), 1);
}

#[tokio::test]
async fn delete_entries_under_escapes_like_wildcards() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();
    let ws = "ws_escape";

    db.entries
        .replace(
            ws,
            "",
            &[
                entry("a%b/x", "x", EntryType::File),
                entry("axxb/x", "x", EntryType::File),
                entry("a_b/y", "y", EntryType::File),
                entry("axb/y", "y", EntryType::File),
            ],
        )
        .await
        .unwrap();

    db.entries.delete_under(ws, "a%b").await.unwrap();
    assert_eq!(
        paths(&db.entries.list(ws, "").await.unwrap()),
        vec!["a_b/y", "axb/y", "axxb/x"]
    );

    db.entries.delete_under(ws, "a_b").await.unwrap();
    assert_eq!(
        paths(&db.entries.list(ws, "").await.unwrap()),
        vec!["axb/y", "axxb/x"]
    );
}
