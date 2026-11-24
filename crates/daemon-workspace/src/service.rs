//! The public workspace service facade.

use std::sync::Arc;

use fleet_core_protocol::workspace::{
    DirectoryListing, FileContent, FileMetadata, FileRange, ListDirectoryRequest,
    ReadFileRangeRequest, ReadFileRequest, RegisterWorkspaceRequest, SearchRequest, SearchResponse,
    Workspace,
};
use fleet_daemon_storage::DaemonStorage;

use crate::directory_reader;
use crate::file_reader;
use crate::file_watcher;
use crate::path_policy::PathPolicy;
use crate::registry;
use crate::search;
use crate::settings::WorkspaceSettings;
use crate::{Result, WorkspaceError};

pub use crate::file_watcher::{WorkspaceWatcher, WorkspaceWatcherStop};

/// Workspace service backed by the shared storage handle.
///
/// Cloning the service is cheap; it contains only the storage handle and the
/// settings.
#[derive(Clone)]
pub struct WorkspaceService {
    storage: Arc<DaemonStorage>,
    settings: WorkspaceSettings,
}

impl WorkspaceService {
    /// Create a service using `storage` for persistence and `settings` for
    /// limits and default ignores.
    pub fn new(storage: Arc<DaemonStorage>, settings: WorkspaceSettings) -> Self {
        Self { storage, settings }
    }

    /// Access the settings used by this service.
    pub fn settings(&self) -> &WorkspaceSettings {
        &self.settings
    }

    /// Access the underlying storage handle.
    pub fn storage(&self) -> &Arc<DaemonStorage> {
        &self.storage
    }

    /// Register a local directory as a workspace.
    pub async fn register(
        &self,
        machine_id: &str,
        request: RegisterWorkspaceRequest,
    ) -> Result<Workspace> {
        registry::register(&self.storage, &self.settings, machine_id, request).await
    }

    /// List registered workspaces, optionally for one machine.
    pub async fn list(&self, machine_id: Option<&str>) -> Result<Vec<Workspace>> {
        registry::list(&self.storage, machine_id).await
    }

    /// Fetch one workspace.
    pub async fn get(&self, workspace_id: &str) -> Result<Workspace> {
        registry::get(&self.storage, workspace_id).await
    }

    /// Remove a workspace and its cached directory index.
    pub async fn remove(&self, workspace_id: &str) -> Result<()> {
        registry::remove(&self.storage, workspace_id).await
    }

    /// List one directory of a workspace.
    ///
    /// The filesystem is authoritative; when a transient filesystem error
    /// occurs and the SQLite cache holds entries for the directory, the cache
    /// is used as a fallback.
    pub async fn list_directory(
        &self,
        workspace_id: &str,
        request: ListDirectoryRequest,
    ) -> Result<DirectoryListing> {
        let policy = self.policy(workspace_id).await?;
        let parent = PathPolicy::normalize_relative(&request.path)?;
        let limit = request
            .limit
            .unwrap_or(directory_reader::DEFAULT_LIMIT)
            .clamp(1, directory_reader::MAX_LIMIT);

        directory_reader::list_directory(
            &self.storage,
            &policy,
            &self.settings,
            workspace_id,
            &parent,
            request.cursor.as_deref(),
            limit,
            request.show_ignored.unwrap_or(false),
        )
        .await
    }

    /// Read one file. `Ok(None)` means the request ETag is still current.
    pub async fn read_file(
        &self,
        workspace_id: &str,
        request: ReadFileRequest,
    ) -> Result<Option<FileContent>> {
        let policy = self.policy(workspace_id).await?;
        file_reader::read_file(&policy, &self.settings, &request).await
    }

    /// Stat one file without returning content.
    pub async fn stat_file(&self, workspace_id: &str, path: &str) -> Result<FileMetadata> {
        let policy = self.policy(workspace_id).await?;
        file_reader::stat_file(&policy, path).await
    }

    /// Read one bounded byte range of a file.
    pub async fn read_file_range(
        &self,
        workspace_id: &str,
        request: ReadFileRangeRequest,
    ) -> Result<FileRange> {
        let policy = self.policy(workspace_id).await?;
        file_reader::read_file_range(&policy, &self.settings, &request).await
    }

    /// Search text files in a workspace.
    pub async fn search(
        &self,
        workspace_id: &str,
        request: SearchRequest,
    ) -> Result<SearchResponse> {
        let policy = self.policy(workspace_id).await?;
        search::search(&policy, &self.settings, &request).await
    }

    /// Start a debounced recursive watcher for a workspace.
    ///
    /// Events are delivered through [`WorkspaceWatcher::events`]; the watcher
    /// also refreshes the cached directory index on a best-effort basis.
    pub async fn watch(&self, workspace_id: &str) -> Result<WorkspaceWatcher> {
        let policy = self.policy(workspace_id).await?;
        file_watcher::start(
            workspace_id.to_string(),
            policy,
            self.settings.clone(),
            self.storage.clone(),
        )
        .await
    }

    async fn policy(&self, workspace_id: &str) -> Result<PathPolicy> {
        let workspace = registry::get(&self.storage, workspace_id).await?;
        match PathPolicy::new(&workspace.root_path) {
            Ok(policy) => Ok(policy),
            Err(WorkspaceError::NotFound(_)) => Err(WorkspaceError::NotFound(format!(
                "workspace root no longer exists: {}",
                workspace.root_path
            ))),
            Err(error) => Err(error),
        }
    }
}
