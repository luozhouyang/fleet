use fleet_admin_storage::session_ui::{SessionUiPatch, LEGACY_ACTOR};
use fleet_admin_storage::AdminStorage;
use fleet_core_common::now;
use fleet_core_protocol::agent::AgentSession;

async fn storage_with_session(id: &str) -> AdminStorage {
    let db = AdminStorage::connect_in_memory().await.unwrap();
    sqlx::query(
        "INSERT INTO machines (id, name, created_at, updated_at) VALUES ('m_1', 'm_1', ?1, ?1)",
    )
    .bind(now().to_rfc3339())
    .execute(db.pool())
    .await
    .unwrap();
    let mut session = AgentSession::new("m_1", "mock", "mock", "/tmp");
    session.id = id.to_string();
    db.sessions.upsert(&session).await.unwrap();
    db
}

#[tokio::test]
async fn pins_archive_and_labels_round_trip_per_actor() {
    let db = storage_with_session("s_1").await;

    let patch = SessionUiPatch {
        pinned: Some(true),
        archived: Some(true),
        labels: Some(vec!["backend".into(), "review".into()]),
        folder_id: None,
    };
    assert!(db.session_ui.patch("user_a", "s_1", &patch).await.unwrap());

    let states = db
        .session_ui
        .get_many("user_a", &["s_1".into()])
        .await
        .unwrap();
    let state = states.get("s_1").expect("state");
    assert!(state.pinned);
    assert!(state.archived);
    assert_eq!(
        state.labels,
        vec!["backend".to_string(), "review".to_string()]
    );
    assert!(state.pinned_rank.is_some());

    // Another actor sees nothing, including the bootstrap admin.
    assert!(db
        .session_ui
        .get_many("user_b", &["s_1".into()])
        .await
        .unwrap()
        .is_empty());
    assert!(db
        .session_ui
        .get_many(LEGACY_ACTOR, &["s_1".into()])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn patches_leave_untouched_fields_alone() {
    let db = storage_with_session("s_1").await;
    db.session_ui
        .patch(
            "user_a",
            "s_1",
            &SessionUiPatch {
                pinned: Some(true),
                labels: Some(vec!["one".into()]),
                ..SessionUiPatch::default()
            },
        )
        .await
        .unwrap();
    db.session_ui
        .patch(
            "user_a",
            "s_1",
            &SessionUiPatch {
                archived: Some(true),
                ..SessionUiPatch::default()
            },
        )
        .await
        .unwrap();

    let states = db
        .session_ui
        .get_many("user_a", &["s_1".into()])
        .await
        .unwrap();
    let state = &states["s_1"];
    assert!(state.pinned, "a later patch must not clear the pin");
    assert!(state.archived);
    assert_eq!(state.labels, vec!["one".to_string()]);
}

#[tokio::test]
async fn patch_rejects_unknown_sessions() {
    let db = storage_with_session("s_1").await;
    let applied = db
        .session_ui
        .patch(
            "user_a",
            "missing",
            &SessionUiPatch {
                pinned: Some(true),
                ..SessionUiPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!applied);
}

#[tokio::test]
async fn pinned_order_is_stored_as_ranks() {
    let db = storage_with_session("s_1").await;
    let mut session = AgentSession::new("m_1", "mock", "mock", "/tmp");
    session.id = "s_2".into();
    db.sessions.upsert(&session).await.unwrap();

    for id in ["s_1", "s_2"] {
        db.session_ui
            .patch(
                "user_a",
                id,
                &SessionUiPatch {
                    pinned: Some(true),
                    ..SessionUiPatch::default()
                },
            )
            .await
            .unwrap();
    }
    db.session_ui
        .replace_pins("user_a", &["s_2".into(), "s_1".into()])
        .await
        .unwrap();

    let states = db
        .session_ui
        .get_many("user_a", &["s_1".into(), "s_2".into()])
        .await
        .unwrap();
    assert_eq!(states["s_2"].pinned_rank, Some(1));
    assert_eq!(states["s_1"].pinned_rank, Some(2));
}

#[tokio::test]
async fn read_watermarks_only_move_forward() {
    let db = storage_with_session("s_1").await;
    let later = now();
    let earlier = later - chrono::Duration::minutes(10);

    db.session_ui
        .mark_read("user_a", &["s_1".into()], later)
        .await
        .unwrap();
    // A stale client cannot rewind the watermark and make old activity unread.
    db.session_ui
        .mark_read("user_a", &["s_1".into()], earlier)
        .await
        .unwrap();

    let watermarks = db
        .session_ui
        .read_watermarks("user_a", &["s_1".into()])
        .await
        .unwrap();
    assert_eq!(watermarks["s_1"], later.to_rfc3339());
}

#[tokio::test]
async fn folders_are_per_actor_and_cascade_to_sessions() {
    let db = storage_with_session("s_1").await;
    let folder = db
        .session_ui
        .create_folder("user_a", "Inbox")
        .await
        .unwrap();
    assert_eq!(db.session_ui.list_folders("user_a").await.unwrap().len(), 1);
    assert!(db
        .session_ui
        .list_folders("user_b")
        .await
        .unwrap()
        .is_empty());

    db.session_ui
        .patch(
            "user_a",
            "s_1",
            &SessionUiPatch {
                folder_id: Some(Some(folder.id.clone())),
                ..SessionUiPatch::default()
            },
        )
        .await
        .unwrap();
    let states = db
        .session_ui
        .get_many("user_a", &["s_1".into()])
        .await
        .unwrap();
    assert_eq!(states["s_1"].folder_id.as_deref(), Some(folder.id.as_str()));

    // Renaming is actor-scoped.
    assert!(!db
        .session_ui
        .rename_folder("user_b", &folder.id, "Nope")
        .await
        .unwrap());
    assert!(db
        .session_ui
        .rename_folder("user_a", &folder.id, "Later")
        .await
        .unwrap());
    assert_eq!(
        db.session_ui.list_folders("user_a").await.unwrap()[0].name,
        "Later"
    );

    // Deleting the folder detaches its sessions.
    assert!(db
        .session_ui
        .delete_folder("user_a", &folder.id)
        .await
        .unwrap());
    let states = db
        .session_ui
        .get_many("user_a", &["s_1".into()])
        .await
        .unwrap();
    assert!(states["s_1"].folder_id.is_none());
}

#[tokio::test]
async fn activity_watermark_only_moves_forward() {
    let db = storage_with_session("s_1").await;
    let later = now();
    let earlier = later - chrono::Duration::minutes(5);

    db.sessions.touch_activity("s_1", later).await.unwrap();
    // A replayed, older event must not rewind the watermark.
    db.sessions.touch_activity("s_1", earlier).await.unwrap();

    let watermarks = db
        .sessions
        .activity_watermarks(&["s_1".into()])
        .await
        .unwrap();
    assert_eq!(watermarks["s_1"], later.to_rfc3339());
}

#[tokio::test]
async fn reconciliation_does_not_clobber_presentation_state() {
    let db = storage_with_session("s_1").await;
    db.session_ui
        .patch(
            "user_a",
            "s_1",
            &SessionUiPatch {
                pinned: Some(true),
                archived: Some(true),
                labels: Some(vec!["keep".into()]),
                ..SessionUiPatch::default()
            },
        )
        .await
        .unwrap();
    let activity = now();
    db.sessions.touch_activity("s_1", activity).await.unwrap();

    // A daemon-style upsert rewrites every session column it owns.
    let mut session = AgentSession::new("m_1", "mock", "mock", "/tmp");
    session.id = "s_1".into();
    session.title = Some("from daemon".into());
    db.sessions.upsert(&session).await.unwrap();

    let states = db
        .session_ui
        .get_many("user_a", &["s_1".into()])
        .await
        .unwrap();
    assert!(states["s_1"].pinned);
    assert!(states["s_1"].archived);
    assert_eq!(states["s_1"].labels, vec!["keep".to_string()]);
    let watermarks = db
        .sessions
        .activity_watermarks(&["s_1".into()])
        .await
        .unwrap();
    assert_eq!(watermarks["s_1"], activity.to_rfc3339());
}
