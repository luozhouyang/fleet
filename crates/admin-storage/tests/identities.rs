use fleet_admin_storage::auth_identities::UnlinkIdentity;
use fleet_admin_storage::AdminStorage;
use fleet_core_common::{new_id, now, Timestamp};

async fn db() -> AdminStorage {
    AdminStorage::connect_in_memory().await.unwrap()
}

/// Create a user with a unique token hash; `expires_at` is `None` for no token.
/// Returns the generated user id.
async fn user(db: &AdminStorage, name: &str, expires_at: Option<Timestamp>) -> String {
    let token_hash = expires_at.map(|_| new_id("hash"));
    db.users
        .create(
            Some(&format!("{name}@example.com")),
            name,
            None,
            token_hash.as_deref(),
            token_hash.as_ref().map(|_| now()),
            expires_at,
        )
        .await
        .unwrap()
        .id
}

async fn link(db: &AdminStorage, user_id: &str, provider: &str, subject: &str) -> String {
    db.auth_identities
        .link(
            user_id,
            provider,
            subject,
            &format!("{subject}@example.com"),
            Some(subject),
            None,
        )
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn unlinking_one_of_two_identities_succeeds() {
    let db = db().await;
    let user_a = user(&db, "user_a", None).await;
    let github = link(&db, &user_a, "github", "gh-1").await;
    link(&db, &user_a, "google", "gg-1").await;

    assert_eq!(
        db.auth_identities.unlink(&user_a, &github).await.unwrap(),
        UnlinkIdentity::Unlinked {
            provider: "github".to_string()
        }
    );
    let remaining = db.auth_identities.for_user(&user_a).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].provider, "google");
}

#[tokio::test]
async fn unlinking_the_last_identity_without_a_token_is_refused() {
    let db = db().await;
    let user_a = user(&db, "user_a", None).await;
    let identity = link(&db, &user_a, "github", "gh-1").await;

    assert_eq!(
        db.auth_identities.unlink(&user_a, &identity).await.unwrap(),
        UnlinkIdentity::LastSignInMethod
    );
    assert_eq!(db.auth_identities.for_user(&user_a).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_usable_token_counts_as_a_sign_in_method() {
    let db = db().await;
    let user_a = user(&db, "user_a", Some(now() + chrono::Duration::days(1))).await;
    let identity = link(&db, &user_a, "github", "gh-1").await;

    assert!(matches!(
        db.auth_identities.unlink(&user_a, &identity).await.unwrap(),
        UnlinkIdentity::Unlinked { .. }
    ));
}

#[tokio::test]
async fn an_expired_token_does_not_unlock_the_last_identity() {
    let db = db().await;
    let user_a = user(&db, "user_a", Some(now() - chrono::Duration::days(1))).await;
    let identity = link(&db, &user_a, "github", "gh-1").await;

    assert_eq!(
        db.auth_identities.unlink(&user_a, &identity).await.unwrap(),
        UnlinkIdentity::LastSignInMethod
    );
}

#[tokio::test]
async fn a_disabled_account_token_does_not_unlock_the_last_identity() {
    let db = db().await;
    let user_a = user(&db, "user_a", Some(now() + chrono::Duration::days(1))).await;
    db.users.disable(&user_a).await.unwrap();
    let identity = link(&db, &user_a, "github", "gh-1").await;

    assert_eq!(
        db.auth_identities.unlink(&user_a, &identity).await.unwrap(),
        UnlinkIdentity::LastSignInMethod
    );
}

#[tokio::test]
async fn another_users_identity_is_not_found() {
    let db = db().await;
    let user_a = user(&db, "user_a", None).await;
    let user_b = user(&db, "user_b", None).await;
    let identity = link(&db, &user_a, "github", "gh-1").await;
    link(&db, &user_a, "google", "gg-1").await;

    assert_eq!(
        db.auth_identities.unlink(&user_b, &identity).await.unwrap(),
        UnlinkIdentity::NotFound
    );
    assert_eq!(db.auth_identities.for_user(&user_a).await.unwrap().len(), 2);
}
