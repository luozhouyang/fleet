use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{Duration, Utc};
use fleet_core_common::ApiError;
use fleet_daemon_storage::{idempotency::Reservation, DaemonStorage};
use serde_json::{json, Value};

async fn storage() -> DaemonStorage {
    DaemonStorage::connect_in_memory().await.unwrap()
}

#[tokio::test]
async fn a_completed_key_replays_without_executing_again() {
    let db = storage().await;
    let calls = Arc::new(AtomicUsize::new(0));

    let first = db
        .idempotency
        .execute("key-1", "git.commit", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<Value, ApiError>(json!({"commit": "abc123"}))
        })
        .await
        .unwrap();
    assert_eq!(first, json!({"commit": "abc123"}));

    let second = db
        .idempotency
        .execute("key-1", "git.commit", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<Value, ApiError>(json!({"commit": "should-not-run"}))
        })
        .await
        .unwrap();

    assert_eq!(second, first);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_error_releases_the_key_for_a_retry() {
    let db = storage().await;
    let calls = Arc::new(AtomicUsize::new(0));

    let first = db
        .idempotency
        .execute("key-2", "agent.permissions.respond", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<Value, ApiError>(ApiError::invalid("permission is already resolved"))
        })
        .await;
    assert!(first.is_err());

    let retried = db
        .idempotency
        .execute("key-2", "agent.permissions.respond", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<Value, ApiError>(json!({"ok": true}))
        })
        .await
        .unwrap();

    assert_eq!(retried, json!({"ok": true}));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_key_is_bound_to_its_method() {
    let db = storage().await;
    db.idempotency
        .execute("key-3", "git.commit", || async {
            Ok::<Value, ApiError>(json!({"ok": true}))
        })
        .await
        .unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let error = db
        .idempotency
        .execute("key-3", "git.checkout", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<Value, ApiError>(json!({"ok": true}))
        })
        .await
        .unwrap_err();

    assert_eq!(error.code, fleet_core_common::error_codes::CONFLICT);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_duplicate_is_rejected_while_the_first_request_runs() {
    let db = storage().await;
    assert_eq!(
        db.idempotency
            .reserve("key-4", "agent.sessions.create")
            .await
            .unwrap(),
        Reservation::Started
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let error = db
        .idempotency
        .execute("key-4", "agent.sessions.create", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<Value, ApiError>(json!({"ok": true}))
        })
        .await
        .unwrap_err();

    assert_eq!(error.code, fleet_core_common::error_codes::CONFLICT);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // Once the first request completes, its response is replayed.
    db.idempotency
        .complete(
            "key-4",
            "agent.sessions.create",
            &json!({"session_id": "s_1"}),
        )
        .await
        .unwrap();
    assert_eq!(
        db.idempotency
            .reserve("key-4", "agent.sessions.create")
            .await
            .unwrap(),
        Reservation::Completed(json!({"session_id": "s_1"}))
    );
}

#[tokio::test]
async fn expired_reservations_and_responses_are_reclaimed() {
    let db = storage().await;
    db.idempotency
        .execute("key-5", "agent.sessions.interrupt", || async {
            Ok::<Value, ApiError>(json!({"ok": true}))
        })
        .await
        .unwrap();

    expire(&db, "key-5").await;
    let calls = Arc::new(AtomicUsize::new(0));
    db.idempotency
        .execute("key-5", "agent.sessions.interrupt", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<Value, ApiError>(json!({"ok": true}))
        })
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prune_removes_only_expired_keys() {
    let db = storage().await;
    db.idempotency
        .execute("expired", "git.commit", || async {
            Ok::<Value, ApiError>(json!({}))
        })
        .await
        .unwrap();
    db.idempotency
        .execute("live", "git.commit", || async {
            Ok::<Value, ApiError>(json!({}))
        })
        .await
        .unwrap();
    expire(&db, "expired").await;

    assert_eq!(db.idempotency.prune().await.unwrap(), 1);
    assert_eq!(
        db.idempotency.reserve("live", "git.commit").await.unwrap(),
        Reservation::Completed(json!({}))
    );
    assert_eq!(
        db.idempotency
            .reserve("expired", "git.commit")
            .await
            .unwrap(),
        Reservation::Started
    );
}

async fn expire(db: &DaemonStorage, key: &str) {
    let past = (Utc::now() - Duration::days(1)).to_rfc3339();
    sqlx::query("UPDATE idempotency_keys SET expires_at = ?2 WHERE key = ?1")
        .bind(key)
        .bind(past)
        .execute(db.pool())
        .await
        .unwrap();
}
