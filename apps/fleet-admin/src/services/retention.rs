//! Scheduled hard deletion of organizations past their retention window.
//!
//! A team organization is soft-deleted immediately: machines are detached and
//! the row becomes invisible. This task removes the row after
//! `retention.deleted_org_grace_days`, which cascades memberships,
//! invitations and pairing codes. Machines, workspaces, agent sessions and
//! audit rows are never deleted.

use std::time::Duration;

use chrono::Duration as ChronoDuration;
use fleet_core_common::now;

use crate::state::AppState;

/// Spawn the retention loop.
///
/// The first sweep runs immediately so an admin that was down past the grace
/// period catches up before the first interval elapses.
pub fn spawn(
    state: AppState,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            if let Err(error) = purge_once(&state).await {
                tracing::warn!(error = %error, "failed to purge expired organizations");
            }
            let interval = Duration::from_secs(state.config.retention.purge_interval_secs.max(1));
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
        tracing::debug!("organization retention task stopped");
    })
}

/// Run one sweep with the configured grace period.
pub(crate) async fn purge_once(state: &AppState) -> fleet_admin_storage::Result<()> {
    let grace_days = state.config.retention.deleted_org_grace_days;
    if grace_days == 0 {
        return Ok(());
    }
    let Some(grace) = i64::try_from(grace_days)
        .ok()
        .and_then(ChronoDuration::try_days)
    else {
        tracing::warn!(
            grace_days,
            "retention grace period is not representable; skipping the purge"
        );
        return Ok(());
    };

    let purged = state.db.organizations.purge_expired(now() - grace).await?;
    for organization in purged {
        tracing::info!(
            org = %organization.id,
            name = %organization.name,
            "purged a soft-deleted organization"
        );
        if let Err(error) = state
            .db
            .audit
            .append(
                "retention",
                None,
                "org.purge",
                &organization.id,
                serde_json::json!({
                    "name": organization.name,
                    "deleted_at": organization.deleted_at.to_rfc3339(),
                }),
            )
            .await
        {
            tracing::warn!(
                org = %organization.id,
                error = %error,
                "failed to audit an organization purge"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fleet_admin_storage::AdminStorage;
    use fleet_core_config::AdminConfig;
    use fleet_core_protocol::authz::OrganizationKind;

    use super::*;

    async fn state(grace_days: u64) -> AppState {
        let mut config = AdminConfig::default();
        config.retention.deleted_org_grace_days = grace_days;
        let db = Arc::new(AdminStorage::connect_in_memory().await.unwrap());
        AppState::new(db, config).await.unwrap()
    }

    /// Soft-delete an organization and backdate its deletion.
    async fn deleted_org(state: &AppState, name: &str, age_days: i64) -> String {
        let organization = state
            .db
            .organizations
            .create(OrganizationKind::Team, name, None)
            .await
            .unwrap();
        state
            .db
            .organizations
            .soft_delete_detaching(&organization.id)
            .await
            .unwrap();
        let deleted_at = now() - chrono::Duration::days(age_days);
        sqlx::query("UPDATE organizations SET deleted_at = ?2 WHERE id = ?1")
            .bind(&organization.id)
            .bind(deleted_at.to_rfc3339())
            .execute(state.db.pool())
            .await
            .unwrap();
        organization.id
    }

    #[tokio::test]
    async fn purges_only_organizations_past_the_grace_period() {
        let state = state(30).await;
        let expired = deleted_org(&state, "Expired", 31).await;
        let recent = deleted_org(&state, "Recent", 1).await;

        purge_once(&state).await.unwrap();

        assert!(state
            .db
            .organizations
            .get(&expired)
            .await
            .unwrap()
            .is_none());
        let remaining = state
            .db
            .organizations
            .get(&recent)
            .await
            .unwrap()
            .expect("a recent deletion is kept");
        assert!(remaining.deleted_at.is_some());

        let audit = state.db.audit.list(None, 10).await.unwrap();
        let entry = audit
            .iter()
            .find(|entry| entry.action == "org.purge")
            .expect("a purge must be audited");
        assert_eq!(entry.actor, "retention");
        assert_eq!(entry.subject, expired);
        assert_eq!(entry.details["name"], "Expired");
        assert!(
            !audit
                .iter()
                .any(|entry| entry.action == "org.purge" && entry.subject == recent),
            "the recent organization must not be audited as purged"
        );
    }

    #[tokio::test]
    async fn a_zero_grace_period_disables_the_sweep() {
        let state = state(0).await;
        let expired = deleted_org(&state, "Expired", 365).await;

        purge_once(&state).await.unwrap();

        assert!(state
            .db
            .organizations
            .get(&expired)
            .await
            .unwrap()
            .is_some());
        assert!(state
            .db
            .audit
            .list(None, 10)
            .await
            .unwrap()
            .iter()
            .all(|entry| entry.action != "org.purge"));
    }
}
