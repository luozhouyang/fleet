mod common;

use common::*;
use fleet_daemon_storage::DaemonStorage;

#[tokio::test]
async fn workspace_upsert_list_and_delete() {
    let db = DaemonStorage::connect_in_memory().await.unwrap();

    let alpha = workspace("ws_1", "m_1", "alpha");
    let beta = workspace("ws_2", "m_2", "beta");
    db.workspaces.upsert(&beta).await.unwrap();
    db.workspaces.upsert(&alpha).await.unwrap();

    assert_eq!(db.workspaces.get("ws_1").await.unwrap().unwrap(), alpha);

    let all = db.workspaces.list(None).await.unwrap();
    assert_eq!(
        all.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );

    let only_m1 = db.workspaces.list(Some("m_1")).await.unwrap();
    assert_eq!(only_m1.len(), 1);
    assert_eq!(only_m1[0].id, "ws_1");

    let mut renamed = alpha.clone();
    renamed.name = "alpha-renamed".to_owned();
    renamed.root_path = "/tmp/renamed".to_owned();
    renamed.updated_at = ts(1_800_000_000);
    db.workspaces.upsert(&renamed).await.unwrap();
    assert_eq!(db.workspaces.get("ws_1").await.unwrap().unwrap(), renamed);

    db.workspaces.delete("ws_2").await.unwrap();
    assert!(db.workspaces.get("ws_2").await.unwrap().is_none());
    assert_eq!(db.workspaces.list(None).await.unwrap().len(), 1);
}
