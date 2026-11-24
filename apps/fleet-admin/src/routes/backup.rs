//! Consistent admin database snapshots.

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Extension;
use chrono::Utc;
use uuid::Uuid;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::state::AppState;

/// `GET /api/v1/admin/backup`
///
/// `VACUUM INTO` produces a consistent snapshot while the server is running.
/// Only the bootstrap token may download it: the file contains every
/// organization's data and user token hashes.
pub async fn download(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, AppError> {
    if !principal.is_legacy_admin() {
        return Err(AppError::forbidden(
            "only the bootstrap admin token can download a backup",
        ));
    }

    // The snapshot contains credentials, so it lives in a private temporary
    // directory and is removed before the response is returned.
    let dir = std::env::temp_dir().join(format!("fleet-admin-backup-{}", Uuid::new_v4()));
    tokio::fs::create_dir(&dir).await.map_err(|error| {
        AppError::internal(format!("failed to create backup directory: {error}"))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    let path = dir.join("fleet-admin.db");
    let literal = path.to_string_lossy().replace('\'', "''");
    let vacuum = sqlx::query(&format!("VACUUM INTO '{literal}'"))
        .execute(state.db.pool())
        .await;
    if let Err(error) = vacuum {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return Err(AppError::internal(format!("backup failed: {error}")));
    }

    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Err(AppError::internal(format!(
                "failed to read the backup: {error}"
            )));
        }
    };
    let _ = tokio::fs::remove_dir_all(&dir).await;

    if let Err(error) = state
        .db
        .audit
        .append(
            &principal.actor,
            None,
            "admin.backup",
            "admin",
            serde_json::json!({ "bytes": bytes.len() }),
        )
        .await
    {
        tracing::warn!(error = %error, "failed to audit backup download");
    }

    let filename = format!("fleet-admin-{}.db", Utc::now().format("%Y%m%d-%H%M%S"));
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.sqlite3"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|_| AppError::internal("failed to build download headers"))?,
    );
    Ok(response)
}
