//! Workspace registry and file/tree proxying.

use agent_fleet_protocol::rpc::{
    methods, FileReadRequest, FileReadResponse, FileSearchRequest, RegisterWorkspaceRpcRequest,
    TreeListRequest, WorkspaceRequest,
};
use agent_fleet_protocol::workspace::{DirectoryListing, SearchResponse, Workspace};

use crate::error::AppError;
use crate::services::{forward, not_found, parse_response, to_payload};
use crate::state::AppState;

/// Workspace operations exposed by the control plane.
pub struct WorkspaceService;

impl WorkspaceService {
    /// List workspaces, optionally filtered by machine.
    pub async fn list(
        state: &AppState,
        machine_id: Option<&str>,
    ) -> Result<Vec<Workspace>, AppError> {
        Ok(state.db.list_workspaces(machine_id).await?)
    }

    /// Look up one workspace or return `404`.
    pub async fn get(state: &AppState, id: &str) -> Result<Workspace, AppError> {
        state
            .db
            .get_workspace(id)
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
        state.db.upsert_workspace(&workspace).await?;
        Ok(workspace)
    }

    /// Remove a workspace from its machine and from local storage.
    pub async fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
        let workspace = Self::get(state, id).await?;
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
        state.db.delete_workspace(id).await?;
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
