//! Workspace registration, lookup and removal.

use agent_fleet_common::{new_id, now};
use agent_fleet_protocol::workspace::{RegisterWorkspaceRequest, Workspace};
use agent_fleet_storage::Database;

use crate::settings::WorkspaceSettings;
use crate::{Result, WorkspaceError};

/// Register a directory as a workspace.
///
/// The directory must exist, be accessible, and canonicalize to the stored
/// root path. The name defaults to the directory's file name.
pub(crate) async fn register(
    storage: &Database,
    settings: &WorkspaceSettings,
    machine_id: &str,
    request: RegisterWorkspaceRequest,
) -> Result<Workspace> {
    if request.root_path.trim().is_empty() {
        return Err(WorkspaceError::InvalidRequest(
            "root_path must not be empty".to_string(),
        ));
    }

    let canonical =
        tokio::fs::canonicalize(&request.root_path)
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                    WorkspaceError::NotFound(request.root_path.clone())
                }
                _ => WorkspaceError::Io(error),
            })?;

    let metadata = tokio::fs::metadata(&canonical).await?;
    if !metadata.is_dir() {
        return Err(WorkspaceError::InvalidRequest(format!(
            "workspace root is not a directory: {}",
            canonical.display()
        )));
    }
    if canonical == std::path::Path::new("/") {
        return Err(WorkspaceError::PathDenied(
            "the filesystem root cannot be registered as a workspace".to_string(),
        ));
    }
    if !settings.allowed_roots.is_empty() {
        let mut allowed = false;
        for root in &settings.allowed_roots {
            if let Ok(root) = tokio::fs::canonicalize(root).await {
                if canonical.starts_with(&root) {
                    allowed = true;
                    break;
                }
            }
        }
        if !allowed {
            return Err(WorkspaceError::PathDenied(format!(
                "workspace root {} is outside the configured allowed roots",
                canonical.display()
            )));
        }
    }

    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            canonical
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| canonical.to_string_lossy().into_owned());

    let is_git_repository = is_git_repository(&canonical).await;
    let timestamp = now();
    let workspace = Workspace {
        id: new_id("ws"),
        machine_id: machine_id.to_string(),
        project_id: request.project_id,
        name,
        root_path: canonical.to_string_lossy().into_owned(),
        is_git_repository,
        created_at: timestamp,
        updated_at: timestamp,
    };

    storage.upsert_workspace(&workspace).await?;
    Ok(workspace)
}

/// List workspaces, optionally filtered to one machine.
pub(crate) async fn list(storage: &Database, machine_id: Option<&str>) -> Result<Vec<Workspace>> {
    Ok(storage.list_workspaces(machine_id).await?)
}

/// Fetch one workspace or report it as missing.
pub(crate) async fn get(storage: &Database, workspace_id: &str) -> Result<Workspace> {
    storage
        .get_workspace(workspace_id)
        .await?
        .ok_or_else(|| WorkspaceError::WorkspaceNotFound(workspace_id.to_string()))
}

/// Remove a workspace and every cached directory entry.
pub(crate) async fn remove(storage: &Database, workspace_id: &str) -> Result<()> {
    // Ensure removal of an unknown workspace is reported consistently.
    let _ = get(storage, workspace_id).await?;
    storage.delete_all_entries(workspace_id).await?;
    storage.delete_workspace(workspace_id).await?;
    Ok(())
}

/// Detect a `.git` directory or worktree file at the root.
async fn is_git_repository(root: &std::path::Path) -> bool {
    match tokio::fs::metadata(root.join(".git")).await {
        Ok(metadata) => metadata.is_dir() || metadata.is_file(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::service::WorkspaceService;
    use crate::settings::WorkspaceSettings;
    use agent_fleet_protocol::workspace::RegisterWorkspaceRequest;
    use std::sync::Arc;

    async fn service() -> (
        tempfile::TempDir,
        WorkspaceService,
        Arc<agent_fleet_storage::Database>,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let database = Arc::new(
            agent_fleet_storage::Database::connect_in_memory()
                .await
                .expect("db"),
        );
        let service = WorkspaceService::new(database.clone(), WorkspaceSettings::default());
        (dir, service, database)
    }

    fn register_request(path: &std::path::Path, name: Option<&str>) -> RegisterWorkspaceRequest {
        RegisterWorkspaceRequest {
            root_path: path.to_string_lossy().into_owned(),
            name: name.map(str::to_string),
            project_id: None,
        }
    }

    #[tokio::test]
    async fn register_list_get_remove() {
        let (dir, service, _database) = service().await;
        let workspace = service
            .register("machine_1", register_request(dir.path(), None))
            .await
            .expect("register");

        assert!(workspace.id.starts_with("ws_"));
        assert_eq!(workspace.machine_id, "machine_1");
        assert_eq!(
            workspace.name,
            dir.path().file_name().unwrap().to_str().unwrap()
        );

        let listed = service.list(Some("machine_1")).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, workspace.id);

        let fetched = service.get(&workspace.id).await.expect("get");
        assert_eq!(fetched.id, workspace.id);

        service.remove(&workspace.id).await.expect("remove");
        assert!(service.get(&workspace.id).await.is_err());
        assert!(service.list(None).await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn register_rejects_roots_outside_the_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let allowed = dir.path().join("allowed");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let database = Arc::new(
            agent_fleet_storage::Database::connect_in_memory()
                .await
                .expect("db"),
        );
        let settings = WorkspaceSettings {
            allowed_roots: vec![allowed.clone()],
            ..Default::default()
        };
        let service = WorkspaceService::new(database, settings);

        service
            .register("machine_1", register_request(&allowed, None))
            .await
            .expect("allowed root");
        let error = service
            .register("machine_1", register_request(&outside, None))
            .await
            .expect_err("outside root must be denied");
        assert!(matches!(error, crate::WorkspaceError::PathDenied(_)));
    }

    #[tokio::test]
    async fn register_rejects_the_filesystem_root() {
        let (_dir, service, _database) = service().await;
        let error = service
            .register(
                "machine_1",
                RegisterWorkspaceRequest {
                    root_path: "/".to_string(),
                    name: None,
                    project_id: None,
                },
            )
            .await
            .expect_err("filesystem root must be denied");
        assert!(matches!(error, crate::WorkspaceError::PathDenied(_)));
    }

    #[tokio::test]
    async fn register_uses_explicit_name() {
        let (dir, service, _database) = service().await;
        let workspace = service
            .register(
                "machine_1",
                register_request(dir.path(), Some("my project")),
            )
            .await
            .expect("register");
        assert_eq!(workspace.name, "my project");
    }

    #[tokio::test]
    async fn register_rejects_missing_directory() {
        let (dir, service, _database) = service().await;
        let missing = dir.path().join("missing");
        let error = service
            .register("machine_1", register_request(&missing, None))
            .await
            .expect_err("missing root");
        assert!(matches!(error, crate::WorkspaceError::NotFound(_)));
    }

    #[tokio::test]
    async fn detects_git_directory_and_git_file() {
        let (dir, service, _database) = service().await;

        let with_dir = dir.path().join("with-dir");
        std::fs::create_dir_all(with_dir.join(".git")).expect("mkdir");
        let workspace = service
            .register("machine_1", register_request(&with_dir, None))
            .await
            .expect("register");
        assert!(workspace.is_git_repository);

        let with_file = dir.path().join("with-file");
        std::fs::create_dir_all(&with_file).expect("mkdir");
        std::fs::write(with_file.join(".git"), b"gitdir: ../elsewhere\n").expect("write");
        let workspace = service
            .register("machine_1", register_request(&with_file, None))
            .await
            .expect("register");
        assert!(workspace.is_git_repository);

        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).expect("mkdir");
        let workspace = service
            .register("machine_1", register_request(&plain, None))
            .await
            .expect("register");
        assert!(!workspace.is_git_repository);
    }

    #[tokio::test]
    async fn remove_deletes_cached_entries() {
        let (dir, service, database) = service().await;
        std::fs::write(dir.path().join("a.txt"), b"a").expect("write");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/b.txt"), b"b").expect("write");

        let workspace = service
            .register("machine_1", register_request(dir.path(), None))
            .await
            .expect("register");
        service
            .list_directory(
                &workspace.id,
                agent_fleet_protocol::workspace::ListDirectoryRequest {
                    path: String::new(),
                    cursor: None,
                    limit: None,
                    show_ignored: None,
                },
            )
            .await
            .expect("list root");
        service
            .list_directory(
                &workspace.id,
                agent_fleet_protocol::workspace::ListDirectoryRequest {
                    path: "src".to_string(),
                    cursor: None,
                    limit: None,
                    show_ignored: None,
                },
            )
            .await
            .expect("list src");

        assert!(!database
            .list_entries(&workspace.id, "")
            .await
            .expect("entries")
            .is_empty());

        service.remove(&workspace.id).await.expect("remove");
        assert!(database
            .list_entries(&workspace.id, "")
            .await
            .expect("entries")
            .is_empty());
        assert!(database
            .list_entries(&workspace.id, "src")
            .await
            .expect("entries")
            .is_empty());
    }
}
