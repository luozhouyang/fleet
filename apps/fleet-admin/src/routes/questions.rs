//! User question answer endpoints.

use axum::extract::{Path, State};
use axum::Extension;
use axum::Json;
use fleet_core_protocol::agent::AgentQuestionAnswer;
use serde::Deserialize;

use crate::error::AppError;
use crate::middleware::auth::Principal;
use crate::services::acl;
use crate::services::agent_service::AgentService;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct AnswerQuestionHttpRequest {
    #[serde(default)]
    pub answers: Option<Vec<AgentQuestionAnswer>>,
    #[serde(default)]
    pub message: Option<String>,
}

/// `POST /api/v1/sessions/{session_id}/questions/{question_id}/answer`
pub async fn answer(
    State(state): State<AppState>,
    Path((session_id, question_id)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
    body: Option<Json<AnswerQuestionHttpRequest>>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &session_id).await?;
    let answers = body.and_then(|Json(body)| body.answers);
    Ok(Json(
        AgentService::respond_question(
            &state,
            &session_id,
            &question_id,
            answers,
            false,
            None,
        )
        .await?,
    ))
}

/// `POST /api/v1/sessions/{session_id}/questions/{question_id}/reject`
pub async fn reject(
    State(state): State<AppState>,
    Path((session_id, question_id)): Path<(String, String)>,
    Extension(principal): Extension<Principal>,
    body: Option<Json<AnswerQuestionHttpRequest>>,
) -> Result<Json<serde_json::Value>, AppError> {
    acl::ensure_session_access(&state, &principal, &session_id).await?;
    let message = body.and_then(|Json(body)| body.message);
    Ok(Json(
        AgentService::respond_question(
            &state,
            &session_id,
            &question_id,
            None,
            true,
            message,
        )
        .await?,
    ))
}
