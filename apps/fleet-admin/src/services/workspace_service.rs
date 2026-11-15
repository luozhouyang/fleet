//! Workspace registry and file/tree proxying.

use fleet_core_protocol::connection::{events as connection_events, PlatformEvent};
use fleet_core_protocol::rpc::{
    methods, FileReadRequest, FileReadResponse, FileSearchRequest, RegisterWorkspaceRpcRequest,
    TreeListRequest, WorkspaceRequest,
};
use fleet_core_protocol::workspace::{
    DirectoryListing, SearchResponse, Workspace, WorkspaceSummary,
};

use crate::error::AppError;
use crate::services::{forward, not_found, parse_response, to_payload};
use crate::state::AppState;

/// Workspace operations exposed by the agent admin.
pub struct WorkspaceService;

impl WorkspaceService {
    /// List workspaces, optionally filtered by machine.
    pub async fn list(
        state: &AppState,
        machine_id: Option<&str>,
    ) -> Result<Vec<Workspace>, AppError> {
        Ok(state.db.workspaces.list(machine_id).await?)
    }

    /// Look up one workspace or return `404`.
    pub async fn get(state: &AppState, id: &str) -> Result<Workspace, AppError> {
        state
            .db
            .workspaces
            .get(id)
            .await?
            .ok_or_else(|| not_found("workspace", id))
    }

    /// Register a directory on a connected machine and persist the result.
    pub async fn register(
        state: &AppState,
        request: RegisterWorkspaceRpcRequest,
    ) -> Result<Workspace, AppError> {
        if request.machine_id.trim().is_empty() {
            return Err(AppError::bad_request("machine_id is required"));
        }
        if request.register.root_path.trim().is_empty() {
            return Err(AppError::bad_request("root_path is required"));
        }

        let payload = to_payload(&request.register)?;
        let value = forward(
            state,
            &request.machine_id,
            methods::WORKSPACES_REGISTER,
            payload,
        )
        .await?;
        let workspace: Workspace = parse_response(value, "workspace.register")?;
        state.db.workspaces.upsert(&workspace).await?;
        refresh_machine_workspaces(state, &workspace.machine_id).await;
        Ok(workspace)
    }

    /// Remove a workspace from its machine and from local storage.
    ///
    /// A workspace with active sessions is rejected: deleting it while an
    /// agent still runs would leave the agent writing to an unregistered
    /// directory.
    pub async fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
        let workspace = Self::get(state, id).await?;
        let active = state.db.sessions.list_active_for_workspace(id).await?;
        if !active.is_empty() {
            return Err(AppError::new(
                axum::http::StatusCode::CONFLICT,
                fleet_core_common::error_codes::CONFLICT,
                format!(
                    "workspace has {} active session(s); close them before deleting the workspace",
                    active.len()
                ),
            ));
        }
        let payload = to_payload(&WorkspaceRequest {
            workspace_id: workspace.id.clone(),
        })?;
        forward(
            state,
            &workspace.machine_id,
            methods::WORKSPACES_REMOVE,
            payload,
        )
        .await?;
        state.db.workspaces.delete(id).await?;
        refresh_machine_workspaces(state, &workspace.machine_id).await;
        Ok(())
    }

    /// Proxy `workspace.tree.list`.
    pub async fn tree(
        state: &AppState,
        id: &str,
        request: TreeListRequest,
    ) -> Result<DirectoryListing, AppError> {
        let workspace = Self::get(state, id).await?;
        let payload = to_payload(&TreeListRequest {
            workspace_id: workspace.id.clone(),
            listing: request.listing,
        })?;
        let value = forward(
            state,
            &workspace.machine_id,
            methods::WORKSPACES_TREE,
            payload,
        )
        .await?;
        parse_response(value, "workspace.tree.list")
    }

    /// Proxy `workspace.file.read`.
    pub async fn read_file(
        state: &AppState,
        id: &str,
        request: FileReadRequest,
    ) -> Result<FileReadResponse, AppError> {
        if request.read.path.trim().is_empty() {
            return Err(AppError::bad_request("path is required"));
        }
        let workspace = Self::get(state, id).await?;
        let payload = to_payload(&FileReadRequest {
            workspace_id: workspace.id.clone(),
            read: request.read,
        })?;
        let value = forward(
            state,
            &workspace.machine_id,
            methods::WORKSPACES_FILE_READ,
            payload,
        )
        .await?;
        parse_response(value, "workspace.file.read")
    }

    /// Proxy `workspace.file.search`.
    pub async fn search(
        state: &AppState,
        id: &str,
        request: FileSearchRequest,
    ) -> Result<SearchResponse, AppError> {
        if request.search.query.trim().is_empty() {
            return Err(AppError::bad_request("query parameter `q` is required"));
        }
        let workspace = Self::get(state, id).await?;
        let payload = to_payload(&FileSearchRequest {
            workspace_id: workspace.id.clone(),
            search: request.search,
        })?;
        let value = forward(
            state,
            &workspace.machine_id,
            methods::WORKSPACES_FILE_SEARCH,
            payload,
        )
        .await?;
        parse_response(value, "workspace.file.search")
    }
}

/// Keep the machine snapshot (and dashboard counters) in sync with the
/// workspace registry, and notify browsers.
async fn refresh_machine_workspaces(state: &AppState, machine_id: &str) {
    let summaries = match state.db.workspaces.list(Some(machine_id)).await {
        Ok(workspaces) => workspaces.iter().map(WorkspaceSummary::from).collect(),
        Err(err) => {
            tracing::warn!(machine = %machine_id, error = %err, "failed to list workspaces");
            return;
        }
    };
    if let Err(err) = state
        .machines
        .update_workspaces(machine_id, summaries)
        .await
    {
        tracing::warn!(machine = %machine_id, error = %err, "failed to update machine workspaces");
    }
    if let Ok(Some(machine)) = state.machines.get(machine_id).await {
        let payload = serde_json::to_value(machine).unwrap_or_default();
        let mut event = PlatformEvent::new(connection_events::MACHINE_STATUS, payload);
        event.machine_id = Some(machine_id.to_string());
        state.events.publish(event);
    }
}
