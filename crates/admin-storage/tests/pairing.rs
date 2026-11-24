use fleet_admin_storage::AdminStorage;
use fleet_core_auth::{generate_pair_code, hash_pair_code};
use fleet_core_common::now;
use fleet_core_protocol::pairing::{PairingCodeStatus, PairingCompletion, PairingOutcome};

async fn db() -> AdminStorage {
    AdminStorage::connect_in_memory().await.unwrap()
}

async fn create_code(db: &AdminStorage, ttl_secs: i64) -> (String, String) {
    create_code_for(db, "org_default", ttl_secs).await
}

async fn create_code_for(db: &AdminStorage, org_id: &str, ttl_secs: i64) -> (String, String) {
    let code = generate_pair_code();
    let hash = hash_pair_code(&code).unwrap();
    let expires_at = now() + chrono::Duration::seconds(ttl_secs);
    let id = db
        .pairing_codes
        .create(org_id, "admin", Some("test"), expires_at, &hash)
        .await
        .unwrap();
    (id, code)
}

fn completion(
    code_id: &str,
    machine_id: &str,
    public_key: &str,
    token_hash: &str,
) -> PairingCompletion {
    PairingCompletion {
        code_id: code_id.to_string(),
        machine_id: machine_id.to_string(),
        public_key: public_key.to_string(),
        token_hash: token_hash.to_string(),
        machine_name: "test-machine".into(),
        hostname: "test.local".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        daemon_version: "0.1.0".into(),
        protocol_version: 1,
        rotation_verified: false,
    }
}

#[tokio::test]
async fn pairing_code_roundtrip_and_single_use() {
    let db = db().await;
    let (id, code) = create_code(&db, 600).await;
    let hash = hash_pair_code(&code).unwrap();

    let status = db.pairing_codes.lookup(&hash, 5).await.unwrap();
    assert!(matches!(status, PairingCodeStatus::Valid(_)));

    let outcome: PairingOutcome = db
        .machines
        .complete_pairing(completion(&id, "m_1", "public-key", "token-hash"), 5)
        .await
        .unwrap();
    assert_eq!(outcome.machine_id, "m_1");
    assert_eq!(outcome.org_id, "org_default");

    assert!(matches!(
        db.pairing_codes.lookup(&hash, 5).await.unwrap(),
        PairingCodeStatus::Used { .. }
    ));
    let second = db
        .machines
        .complete_pairing(completion(&id, "m_1", "public-key", "token-hash"), 5)
        .await;
    assert_eq!(second.unwrap_err().code, "pairing_code_invalid");
}

#[tokio::test]
async fn expired_and_locked_codes_are_rejected() {
    let db = db().await;
    let (id, code) = create_code(&db, -1).await;
    let hash = hash_pair_code(&code).unwrap();
    assert!(matches!(
        db.pairing_codes.lookup(&hash, 5).await.unwrap(),
        PairingCodeStatus::Expired { .. }
    ));

    let (id2, code2) = create_code(&db, 600).await;
    let hash2 = hash_pair_code(&code2).unwrap();
    for _ in 0..5 {
        db.pairing_codes.record_failure(&id2).await.unwrap();
    }
    assert!(matches!(
        db.pairing_codes.lookup(&hash2, 5).await.unwrap(),
        PairingCodeStatus::Locked { .. }
    ));
    let _ = id;
}

#[tokio::test]
async fn token_lookup_and_revocation() {
    let db = db().await;
    let (id, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id, "m_1", "public-key", "token-hash"), 5)
        .await
        .unwrap();

    assert_eq!(
        db.machines
            .by_token_hash("token-hash")
            .await
            .unwrap()
            .as_deref(),
        Some("m_1")
    );
    assert_eq!(
        db.machines
            .machine_state("m_1")
            .await
            .unwrap()
            .pinned_key
            .as_deref(),
        Some("public-key")
    );

    db.machines.revoke("m_1", Some("test")).await.unwrap();
    let (id2, _) = create_code(&db, 600).await;
    let err = db
        .machines
        .complete_pairing(completion(&id2, "m_1", "public-key", "token-hash-2"), 5)
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
}

#[tokio::test]
async fn key_mismatch_is_rejected() {
    let db = db().await;
    let (id, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id, "m_1", "key-a", "token-a"), 5)
        .await
        .unwrap();

    let (id2, _) = create_code(&db, 600).await;
    let err = db
        .machines
        .complete_pairing(completion(&id2, "m_1", "key-b", "token-b"), 5)
        .await
        .unwrap_err();
    assert_eq!(err.code, "machine_key_mismatch");
}

#[tokio::test]
async fn org_mismatch_is_rejected() {
    let db = db().await;
    let (id, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id, "m_1", "key-a", "token-a"), 5)
        .await
        .unwrap();

    let other = db
        .organizations
        .create(
            fleet_core_protocol::authz::OrganizationKind::Team,
            "Other",
            None,
        )
        .await
        .unwrap();
    let (id2, _) = create_code_for(&db, &other.id, 600).await;
    let err = db
        .machines
        .complete_pairing(completion(&id2, "m_1", "key-a", "token-b"), 5)
        .await
        .unwrap_err();
    assert_eq!(err.code, "machine_org_mismatch");
}

#[tokio::test]
async fn successful_pairing_writes_an_audit_row() {
    let db = db().await;
    let (id, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id, "m_1", "key-a", "token-a"), 5)
        .await
        .unwrap();

    let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT actor, org_id, action, subject FROM audit_logs WHERE action = 'machine.paired'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "admin");
    assert_eq!(rows[0].1.as_deref(), Some("org_default"));
    assert_eq!(rows[0].2, "machine.paired");
    assert_eq!(rows[0].3, "m_1");

    let (details_json,): (String,) =
        sqlx::query_as("SELECT details_json FROM audit_logs WHERE action = 'machine.paired'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let details: serde_json::Value = serde_json::from_str(&details_json).unwrap();
    let fingerprint = details["public_key_fingerprint"].as_str().unwrap();
    assert_ne!(fingerprint, "key-a");
    assert_eq!(fingerprint, fleet_core_auth::hash_token("key-a"));
}

#[tokio::test]
async fn failed_attempts_are_counted_and_audited() {
    let db = db().await;
    let (id, code) = create_code(&db, 600).await;

    db.pairing_codes.record_failure(&id).await.unwrap();

    let hash = hash_pair_code(&code).unwrap();
    match db.pairing_codes.lookup(&hash, 5).await.unwrap() {
        PairingCodeStatus::Valid(info) => assert_eq!(info.attempts_left, 4),
        other => panic!("expected a valid code, got {other:?}"),
    }

    let rows: Vec<(String, Option<String>, String, String, String)> = sqlx::query_as(
        "SELECT actor, org_id, action, subject, details_json FROM audit_logs \
         WHERE action = 'machine.pair_denied'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "admin");
    assert_eq!(rows[0].1.as_deref(), Some("org_default"));
    assert_eq!(rows[0].2, "machine.pair_denied");
    assert_eq!(rows[0].3, id);
    let details: serde_json::Value = serde_json::from_str(&rows[0].4).unwrap();
    assert_eq!(details["code_id"].as_str(), Some(id.as_str()));
    assert_eq!(details["outcome"], "denied");
    assert_eq!(details["reason"], "signature");
}

#[tokio::test]
async fn unknown_code_failure_writes_no_audit_row() {
    let db = db().await;

    db.pairing_codes
        .record_failure("pair_missing")
        .await
        .unwrap();

    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM audit_logs WHERE action = 'machine.pair_denied'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn re_pairing_rotates_the_token_hash() {
    let db = db().await;
    let (id, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id, "m_1", "key-a", "token-old"), 5)
        .await
        .unwrap();

    let (id2, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id2, "m_1", "key-a", "token-new"), 5)
        .await
        .unwrap();

    assert_eq!(
        db.machines
            .by_token_hash("token-new")
            .await
            .unwrap()
            .as_deref(),
        Some("m_1")
    );
    assert_eq!(db.machines.by_token_hash("token-old").await.unwrap(), None);
}

#[tokio::test]
async fn a_code_cannot_pair_a_second_machine_sequentially() {
    let db = db().await;
    let (id, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id, "m_1", "key-a", "token-a"), 5)
        .await
        .unwrap();

    let err = db
        .machines
        .complete_pairing(completion(&id, "m_2", "key-b", "token-b"), 5)
        .await
        .unwrap_err();
    assert_eq!(err.code, "pairing_code_invalid");
}

#[tokio::test]
async fn rotation_verified_replaces_the_pinned_key() {
    let db = db().await;
    let (id, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id, "m_1", "key-a", "token-a"), 5)
        .await
        .unwrap();

    let (id2, _) = create_code(&db, 600).await;
    let mut rotated = completion(&id2, "m_1", "key-b", "token-b");
    rotated.rotation_verified = true;
    db.machines.complete_pairing(rotated, 5).await.unwrap();

    assert_eq!(
        db.machines
            .machine_state("m_1")
            .await
            .unwrap()
            .pinned_key
            .as_deref(),
        Some("key-b")
    );
    assert_eq!(
        db.machines
            .by_token_hash("token-b")
            .await
            .unwrap()
            .as_deref(),
        Some("m_1")
    );
    assert_eq!(db.machines.by_token_hash("token-a").await.unwrap(), None);
}

#[tokio::test]
async fn complete_pairing_rechecks_expiry_and_attempts() {
    let db = db().await;
    let (expired_id, _) = create_code(&db, -1).await;
    let err = db
        .machines
        .complete_pairing(completion(&expired_id, "m_1", "key-a", "token-a"), 5)
        .await
        .unwrap_err();
    assert_eq!(err.code, "pairing_code_invalid");

    let (locked_id, _) = create_code(&db, 600).await;
    for _ in 0..5 {
        db.pairing_codes.record_failure(&locked_id).await.unwrap();
    }
    let err = db
        .machines
        .complete_pairing(completion(&locked_id, "m_1", "key-a", "token-a"), 5)
        .await
        .unwrap_err();
    assert_eq!(err.code, "pairing_code_invalid");
}

#[tokio::test]
async fn code_creation_and_audit_are_written_together() {
    let db = db().await;
    let code = generate_pair_code();
    let hash = hash_pair_code(&code).unwrap();
    let id = db
        .pairing_codes
        .create_audited(
            "org_default",
            "admin",
            Some("hint-name"),
            now() + chrono::Duration::seconds(60),
            &hash,
        )
        .await
        .unwrap();

    let (action, subject, details_json): (String, String, String) = sqlx::query_as(
        "SELECT action, subject, details_json FROM audit_logs WHERE action = 'pairing.code.create'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(action, "pairing.code.create");
    assert_eq!(subject, id);
    let details: serde_json::Value = serde_json::from_str(&details_json).unwrap();
    assert_eq!(details["code_id"].as_str(), Some(id.as_str()));
    assert_eq!(details["org_id"].as_str(), Some("org_default"));
    assert_eq!(details["machine_name_hint"].as_str(), Some("hint-name"));
    assert!(details["expires_at"].is_string());
}

#[tokio::test]
async fn revocation_destroys_the_stored_token() {
    let db = db().await;
    let (id, _) = create_code(&db, 600).await;
    db.machines
        .complete_pairing(completion(&id, "m_1", "key-a", "token-a"), 5)
        .await
        .unwrap();
    assert_eq!(
        db.machines
            .by_token_hash("token-a")
            .await
            .unwrap()
            .as_deref(),
        Some("m_1")
    );

    // Simulate the delete step not running (crash after revocation commits).
    db.machines.revoke("m_1", Some("test")).await.unwrap();
    assert_eq!(db.machines.by_token_hash("token-a").await.unwrap(), None);

    // Unrevoking must not revive it.
    assert!(db.machines.unrevoke("m_1").await.unwrap());
    assert_eq!(db.machines.by_token_hash("token-a").await.unwrap(), None);
}

#[tokio::test]
async fn concurrent_pairing_has_exactly_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let db = AdminStorage::connect(dir.path().join("admin.db"))
        .await
        .unwrap();
    let (id, _) = create_code(&db, 600).await;

    let first = db
        .machines
        .complete_pairing(completion(&id, "m_1", "key-a", "token-a"), 5);
    let second = db
        .machines
        .complete_pairing(completion(&id, "m_2", "key-b", "token-b"), 5);
    let (first, second) = tokio::join!(first, second);
    match (first, second) {
        (Ok(_), Err(err)) | (Err(err), Ok(_)) => {
            assert_eq!(err.code, "pairing_code_invalid");
        }
        (first, second) => {
            panic!("expected exactly one winner, got {first:?} and {second:?}");
        }
    }
}
