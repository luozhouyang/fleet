//! The public workspace service facade.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use fleet_core_protocol::workspace::{
    DirectoryListing, FileContent, FileMetadata, FileRange, ListDirectoryRequest,
    ReadFileRangeRequest, ReadFileRequest, RegisterWorkspaceRequest, SearchRequest, SearchResponse,
    Workspace,
};
use fleet_daemon_storage::DaemonStorage;
use parking_lot::Mutex;

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
    /// Canonicalized roots keyed by workspace id. Building a policy statically
    /// canonicalizes the root, so caching it keeps per-request path resolution
    /// off the blocking filesystem path.
    policies: Arc<Mutex<HashMap<String, PathPolicy>>>,
}

impl WorkspaceService {
    /// Create a service using `storage` for persistence and `settings` for
    /// limits and default ignores.
    pub fn new(storage: Arc<DaemonStorage>, settings: WorkspaceSettings) -> Self {
        Self {
            storage,
            settings,
            policies: Arc::new(Mutex::new(HashMap::new())),
        }
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
        let workspace =
            registry::register(&self.storage, &self.settings, machine_id, request).await?;
        // Registration changes which roots exist for this machine.
        self.policies.lock().clear();
        Ok(workspace)
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
        registry::remove(&self.storage, workspace_id).await?;
        self.policies.lock().clear();
        Ok(())
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
        let configured_root = Path::new(&workspace.root_path);
        if let Some(policy) = self.policies.lock().get(&workspace.id) {
            if policy.root() == configured_root {
                return Ok(policy.clone());
            }
        }

        // Canonicalizing the root is blocking; build a missing cache entry off
        // the async runtime instead of stalling the worker thread.
        let root = workspace.root_path.clone();
        let policy = tokio::task::spawn_blocking(move || PathPolicy::new(&root))
            .await
            .map_err(|error| {
                WorkspaceError::InvalidRequest(format!(
                    "workspace root validation task failed: {error}"
                ))
            })?;
        match policy {
            Ok(policy) => {
                self.policies.lock().insert(workspace.id, policy.clone());
                Ok(policy)
            }
            Err(WorkspaceError::NotFound(_)) => Err(WorkspaceError::NotFound(format!(
                "workspace root no longer exists: {}",
                workspace.root_path
            ))),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core_protocol::workspace::ListDirectoryRequest;

    fn list_request() -> ListDirectoryRequest {
        ListDirectoryRequest {
            path: String::new(),
            cursor: None,
            limit: None,
            show_ignored: None,
        }
    }

    #[tokio::test]
    async fn policy_cache_follows_the_stored_root() {
        let first = tempfile::tempdir().expect("first");
        let second = tempfile::tempdir().expect("second");
        std::fs::write(first.path().join("a.txt"), b"a").expect("write");
        std::fs::write(second.path().join("b.txt"), b"b").expect("write");

        let database = Arc::new(DaemonStorage::connect_in_memory().await.expect("db"));
        let service = WorkspaceService::new(database.clone(), WorkspaceSettings::default());
        let workspace = service
            .register(
                "m1",
                RegisterWorkspaceRequest {
                    root_path: first.path().to_string_lossy().into_owned(),
                    name: None,
                    project_id: None,
                },
            )
            .await
            .expect("register");

        let listing = service
            .list_directory(&workspace.id, list_request())
            .await
            .expect("first listing");
        assert_eq!(listing.entries[0].name, "a.txt");

        // Point the stored workspace at another directory. The cached policy
        // must notice the new configured root instead of serving the old one.
        let mut moved = workspace.clone();
        moved.root_path = second.path().to_string_lossy().into_owned();
        moved.updated_at = fleet_core_common::now();
        database.workspaces.upsert(&moved).await.expect("update");

        let listing = service
            .list_directory(&workspace.id, list_request())
            .await
            .expect("second listing");
        assert_eq!(listing.entries[0].name, "b.txt");
    }
}
