mod common;

use common::*;
use fleet_admin_storage::AdminStorage;
use fleet_core_common::new_id;
use fleet_core_protocol::agent::SessionStatus;
use fleet_core_protocol::machine::{MachineStatus, RevokedMachine};
use serde_json::json;

#[tokio::test]
async fn machine_roundtrip_status_list_and_delete() {
    let db = AdminStorage::connect_in_memory().await.unwrap();

    let alpha = machine("m_1", "alpha");
    let beta = machine("m_2", "beta");
    db.machines.upsert(&beta).await.unwrap();
    db.machines.upsert(&alpha).await.unwrap();

    let fetched = db.machines.get("m_1").await.unwrap().unwrap();
    assert_eq!(fetched, alpha);

    let listed = db.machines.list().await.unwrap();
    assert_eq!(
        listed.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );

    db.machines
        .update_status(
            "m_1",
            MachineStatus::Degraded,
            None,
            Some(ts(1_800_000_000)),
        )
        .await
        .unwrap();

    let updated = db.machines.get("m_1").await.unwrap().unwrap();
    assert_eq!(updated.status, MachineStatus::Degraded);
    assert_eq!(updated.connected_at, alpha.connected_at);
    assert_eq!(updated.last_seen_at, Some(ts(1_800_000_000)));
    assert!(updated.updated_at > alpha.updated_at);

    // Upsert must replace mutable fields while preserving the nested providers.
    let mut renamed = alpha.clone();
    renamed.name = "alpha-renamed".to_owned();
    renamed.providers[0].models.push("extra-model".to_owned());
    renamed.updated_at = ts(1_800_000_100);
    db.machines.upsert(&renamed).await.unwrap();
    let replaced = db.machines.get("m_1").await.unwrap().unwrap();
    assert_eq!(replaced.name, "alpha-renamed");
    assert_eq!(replaced.providers[0].models.len(), 3);
    assert_eq!(replaced.created_at, alpha.created_at);

    db.machines.delete("m_1").await.unwrap();
    assert!(db.machines.get("m_1").await.unwrap().is_none());
    assert_eq!(db.machines.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn malformed_json_columns_degrade_to_defaults() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let machine = machine("m_1", "alpha");
    db.machines.upsert(&machine).await.unwrap();

    sqlx::query("UPDATE machines SET providers_json = ?1, workspaces_json = ?2 WHERE id = ?3")
        .bind("{not json")
        .bind("also not json")
        .bind("m_1")
        .execute(db.pool())
        .await
        .unwrap();

    let fetched = db.machines.get("m_1").await.unwrap().unwrap();
    assert!(fetched.providers.is_empty());
    assert!(fetched.workspaces.is_empty());
    assert_eq!(fetched.workspace_count, 0);
}

#[tokio::test]
async fn machine_revocation_roundtrip() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    assert!(!db.machines.is_revoked("m_revoked").await.unwrap());
    db.machines
        .revoke("m_revoked", Some("removed by admin"))
        .await
        .unwrap();
    assert!(db.machines.is_revoked("m_revoked").await.unwrap());
    let revoked: Vec<RevokedMachine> = db.machines.list_revoked().await.unwrap();
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0].machine_id, "m_revoked");
    assert_eq!(revoked[0].reason.as_deref(), Some("removed by admin"));
    assert!(db.machines.unrevoke("m_revoked").await.unwrap());
    assert!(!db.machines.is_revoked("m_revoked").await.unwrap());
    assert!(!db.machines.unrevoke("m_revoked").await.unwrap());
}

#[tokio::test]
async fn revocations_are_scoped_to_the_owning_organization() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let org_a = db.organizations.create("Alpha").await.unwrap();
    let org_b = db.organizations.create("Beta").await.unwrap();

    for (machine_id, org_id) in [("m_a", &org_a.id), ("m_b", &org_b.id)] {
        db.machines
            .upsert(&machine(machine_id, machine_id))
            .await
            .unwrap();
        db.machine_orgs.assign(machine_id, org_id).await.unwrap();
        db.machines
            .revoke(machine_id, Some("removed by admin"))
            .await
            .unwrap();
        db.machines.delete(machine_id).await.unwrap();
    }

    let revocations = db
        .machines
        .list_revoked_scoped(Some(&org_a.id))
        .await
        .unwrap();
    assert_eq!(revocations.len(), 1);
    assert_eq!(revocations[0].machine_id, "m_a");
    assert_eq!(
        db.machines.revocation_org("m_a").await.unwrap().as_deref(),
        Some(org_a.id.as_str())
    );
    assert_eq!(
        db.machines
            .revocation_public_key("m_a")
            .await
            .unwrap()
            .as_deref(),
        Some("public-key")
    );

    // An organization admin cannot unrevoke another organization's machine.
    assert!(!db
        .machines
        .unrevoke_scoped("m_b", Some(&org_a.id))
        .await
        .unwrap());
    assert!(db.machines.is_revoked("m_b").await.unwrap());
    assert!(db
        .machines
        .unrevoke_scoped("m_b", Some(&org_b.id))
        .await
        .unwrap());
}

#[tokio::test]
async fn concurrent_reads_do_not_fail() {
    let dir = tempfile::tempdir().unwrap();
    let db = AdminStorage::connect(dir.path().join("fleet-admin.db"))
        .await
        .unwrap();

    for index in 0..3 {
        let machine = machine(&format!("m_{index}"), &format!("machine-{index}"));
        db.machines.upsert(&machine).await.unwrap();
    }

    let mut handles = Vec::new();
    for task in 0..4 {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            for round in 0..25 {
                let machines = db.machines.list().await?;
                assert_eq!(machines.len(), 3, "task {task} round {round}");
            }
            Ok::<(), fleet_admin_storage::StorageError>(())
        }));
    }

    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // Writes still work after the concurrent read burst.
    let machine = machine(&new_id("machine"), "after-reads");
    db.machines.upsert(&machine).await.unwrap();
    assert_eq!(db.machines.list().await.unwrap().len(), 4);
}

#[tokio::test]
async fn deleting_a_machine_removes_dependent_rows() {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    let machine = machine("m_1", "alpha");
    db.machines.upsert(&machine).await.unwrap();
    db.machine_orgs.assign("m_1", "org_default").await.unwrap();
    let workspace = workspace("ws_1", "m_1", "alpha");
    db.workspaces.upsert(&workspace).await.unwrap();
    let session = session(
        "session_1",
        "m_1",
        ts(1_700_000_000),
        SessionStatus::WaitingForInput,
        json!({}),
    );
    db.sessions.upsert(&session).await.unwrap();
    db.machines.delete("m_1").await.unwrap();

    assert!(db.machines.get("m_1").await.unwrap().is_none());
    assert!(db.workspaces.get("ws_1").await.unwrap().is_none());
    assert!(db.sessions.get("session_1").await.unwrap().is_none());
    assert_eq!(db.machine_orgs.get("m_1").await.unwrap(), None);
}
