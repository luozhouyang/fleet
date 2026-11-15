//! Debounced filesystem watcher that publishes workspace events.
//!
//! Raw `notify` events are bridged into a Tokio channel, batched for
//! [`DEBOUNCE`], deduplicated, mapped to protocol events, and published on a
//! broadcast channel. Each batch also refreshes the SQLite index for the
//! affected parent directories on a best-effort basis.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fleet_core_common::{new_id, now};
use fleet_core_protocol::workspace::{WorkspaceEvent, WorkspaceEventType};
use fleet_daemon_storage::DaemonStorage;
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{broadcast, mpsc, watch};

use crate::path_policy::PathPolicy;
use crate::settings::WorkspaceSettings;
use crate::{Result, WorkspaceError};

/// Time to wait for the filesystem to settle before processing a batch.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Broadcast buffer size for workspace events.
const EVENT_CAPACITY: usize = 512;

/// Raw channel buffer for notify events.
const RAW_CAPACITY: usize = 1024;

/// Maximum time a continuous stream of events may postpone a flush.
const MAX_BATCH_DELAY: Duration = Duration::from_secs(1);

/// Flush immediately once this many raw events are buffered.
const MAX_PENDING_EVENTS: usize = 4096;

/// A running workspace watcher.
///
/// Events are received through [`WorkspaceWatcher::events`]. Dropping the
/// watcher or calling [`WorkspaceWatcher::stop`] shuts it down.
pub struct WorkspaceWatcher {
    /// Broadcast receiver for workspace change events.
    pub events: broadcast::Receiver<WorkspaceEvent>,
    stop_tx: Option<watch::Sender<bool>>,
    task: Option<tokio::task::JoinHandle<()>>,
    /// Keeping the notify watcher alive keeps the OS watch registered.
    watcher: Option<RecommendedWatcher>,
}

/// Ownership of a running watcher's lifecycle, without its event receiver.
pub struct WorkspaceWatcherStop {
    stop_tx: Option<watch::Sender<bool>>,
    task: Option<tokio::task::JoinHandle<()>>,
    _watcher: RecommendedWatcher,
}

impl WorkspaceWatcherStop {
    /// Stop the watcher and wait for its background task to finish.
    pub async fn stop(mut self) {
        if let Some(sender) = self.stop_tx.take() {
            let _ = sender.send(true);
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for WorkspaceWatcherStop {
    fn drop(&mut self) {
        if let Some(sender) = self.stop_tx.take() {
            let _ = sender.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl WorkspaceWatcher {
    /// Split the watcher into its event stream and a stop handle.
    pub fn into_parts(mut self) -> (broadcast::Receiver<WorkspaceEvent>, WorkspaceWatcherStop) {
        let stop = WorkspaceWatcherStop {
            stop_tx: self.stop_tx.take(),
            task: self.task.take(),
            _watcher: self.watcher.take().expect("watcher is present until split"),
        };
        let (_dummy_tx, dummy_rx) = broadcast::channel(1);
        let events = std::mem::replace(&mut self.events, dummy_rx);
        (events, stop)
    }

    /// Stop the watcher and wait for its background task to finish.
    pub async fn stop(self) {
        let (events, stop) = self.into_parts();
        drop(events);
        stop.stop().await;
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        if let Some(sender) = self.stop_tx.take() {
            let _ = sender.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Create and start a watcher for `policy.root()`.
pub(crate) async fn start(
    workspace_id: String,
    policy: PathPolicy,
    settings: WorkspaceSettings,
    storage: Arc<DaemonStorage>,
) -> Result<WorkspaceWatcher> {
    let root = policy.root().to_path_buf();
    let (raw_tx, raw_rx) = mpsc::channel::<Event>(RAW_CAPACITY);

    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        // Malformed events and errors are dropped; the watcher stays alive.
        if let Ok(event) = result {
            let _ = raw_tx.try_send(event);
        }
    })
    .map_err(|error| {
        WorkspaceError::InvalidRequest(format!("failed to create file watcher: {error}"))
    })?;

    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|error| {
            WorkspaceError::InvalidRequest(format!(
                "failed to watch workspace root {}: {error}",
                root.display()
            ))
        })?;

    let (event_tx, event_rx) = broadcast::channel(EVENT_CAPACITY);
    let (stop_tx, stop_rx) = watch::channel(false);

    let task = tokio::spawn(run(
        workspace_id,
        policy,
        settings,
        storage,
        raw_rx,
        event_tx,
        stop_rx,
    ));

    Ok(WorkspaceWatcher {
        events: event_rx,
        stop_tx: Some(stop_tx),
        task: Some(task),
        watcher: Some(watcher),
    })
}

#[allow(clippy::too_many_arguments)]
async fn run(
    workspace_id: String,
    policy: PathPolicy,
    settings: WorkspaceSettings,
    storage: Arc<DaemonStorage>,
    mut raw_rx: mpsc::Receiver<Event>,
    event_tx: broadcast::Sender<WorkspaceEvent>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let mut pending: Vec<Event> = Vec::new();
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut batch_started: Option<tokio::time::Instant> = None;

    loop {
        if let Some(when) = deadline {
            tokio::select! {
                _ = stop_rx.changed() => break,
                _ = tokio::time::sleep_until(when) => {
                    flush_batch(
                        &workspace_id,
                        &policy,
                        &settings,
                        &storage,
                        &event_tx,
                        &pending,
                    )
                    .await;
                    pending.clear();
                    deadline = None;
                    batch_started = None;
                }
                maybe = raw_rx.recv() => {
                    match maybe {
                        Some(event) => {
                            pending.push(event);
                            deadline = Some(next_deadline(&mut batch_started, pending.len()));
                        }
                        None => break,
                    }
                }
            }
        } else {
            tokio::select! {
                _ = stop_rx.changed() => break,
                maybe = raw_rx.recv() => {
                    match maybe {
                        Some(event) => {
                            pending.push(event);
                            deadline = Some(next_deadline(&mut batch_started, pending.len()));
                        }
                        None => break,
                    }
                }
            }
        }
    }

    if !pending.is_empty() {
        flush_batch(
            &workspace_id,
            &policy,
            &settings,
            &storage,
            &event_tx,
            &pending,
        )
        .await;
    }
}

/// Debounce relative to the last event but never postpone a batch past
/// [`MAX_BATCH_DELAY`]; a full buffer flushes immediately.
fn next_deadline(
    batch_started: &mut Option<tokio::time::Instant>,
    pending: usize,
) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    let started = *batch_started.get_or_insert(now);
    if pending >= MAX_PENDING_EVENTS {
        return now;
    }
    (now + DEBOUNCE).min(started + MAX_BATCH_DELAY)
}

async fn flush_batch(
    workspace_id: &str,
    policy: &PathPolicy,
    settings: &WorkspaceSettings,
    storage: &DaemonStorage,
    event_tx: &broadcast::Sender<WorkspaceEvent>,
    raw_events: &[Event],
) {
    let root = policy.root();
    let mut seen: HashSet<(String, String, Option<String>)> = HashSet::new();
    let mut parents: HashSet<String> = HashSet::new();
    let mut deletions: Vec<String> = Vec::new();

    for raw in raw_events {
        for event in map_event(workspace_id, root, raw) {
            let key = (
                event.event_type.as_str().to_string(),
                event.path.clone().unwrap_or_default(),
                event.old_path.clone(),
            );
            if !seen.insert(key) {
                continue;
            }

            if let Some(path) = &event.path {
                parents.insert(parent_relative(path));
            }
            if let Some(old_path) = &event.old_path {
                parents.insert(parent_relative(old_path));
                deletions.push(old_path.clone());
            }
            if event.event_type == WorkspaceEventType::FileDeleted {
                if let Some(path) = &event.path {
                    deletions.push(path.clone());
                }
            }

            let _ = event_tx.send(event);
        }

        if matches!(raw.kind, EventKind::Remove(_)) {
            if let Some(path) = raw.paths.first().and_then(|path| relative_path(root, path)) {
                deletions.push(path);
            }
        }
    }

    for path in deletions {
        if let Err(error) = storage.entries.delete_under(workspace_id, &path).await {
            tracing::warn!(
                workspace_id,
                path,
                error = %error,
                "failed to invalidate workspace index"
            );
        }
    }

    for parent in parents {
        match crate::directory_reader::read_directory_entries(policy, settings, &parent, false)
            .await
        {
            Ok(entries) => {
                if let Err(error) = storage
                    .entries
                    .replace(workspace_id, &parent, &entries)
                    .await
                {
                    tracing::warn!(
                        workspace_id,
                        parent,
                        error = %error,
                        "failed to refresh workspace index after change"
                    );
                }
            }
            Err(error) => {
                tracing::debug!(
                    workspace_id,
                    parent,
                    error = %error,
                    "skipping index refresh for directory"
                );
            }
        }
    }
}

fn map_event(workspace_id: &str, root: &Path, event: &Event) -> Vec<WorkspaceEvent> {
    match event.kind {
        EventKind::Access(_) => Vec::new(),
        EventKind::Modify(ModifyKind::Name(_)) => map_rename(workspace_id, root, event),
        _ => map_single(workspace_id, root, event),
    }
}

fn map_single(workspace_id: &str, root: &Path, event: &Event) -> Vec<WorkspaceEvent> {
    let Some(path) = event.paths.first() else {
        return Vec::new();
    };
    let Some(relative) = relative_path(root, path) else {
        return Vec::new();
    };

    let is_dir = std::fs::metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);

    let event_type = match event.kind {
        EventKind::Create(_) => {
            if is_dir {
                WorkspaceEventType::DirectoryChanged
            } else {
                WorkspaceEventType::FileCreated
            }
        }
        EventKind::Modify(ModifyKind::Data(_)) | EventKind::Modify(ModifyKind::Any) => {
            if is_dir {
                WorkspaceEventType::DirectoryChanged
            } else {
                WorkspaceEventType::FileModified
            }
        }
        EventKind::Remove(_) => {
            if is_dir {
                WorkspaceEventType::DirectoryChanged
            } else {
                WorkspaceEventType::FileDeleted
            }
        }
        _ => WorkspaceEventType::DirectoryChanged,
    };

    vec![make_event(workspace_id, event_type, Some(relative), None)]
}

fn map_rename(workspace_id: &str, root: &Path, event: &Event) -> Vec<WorkspaceEvent> {
    if event.paths.len() >= 2 {
        let old = relative_path(root, &event.paths[0]);
        let new = relative_path(root, &event.paths[1]);
        let new_is_dir = std::fs::metadata(&event.paths[1])
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);

        match (old, new) {
            (Some(old), Some(new)) => {
                let event_type = if new_is_dir {
                    WorkspaceEventType::DirectoryChanged
                } else {
                    WorkspaceEventType::FileRenamed
                };
                vec![make_event(workspace_id, event_type, Some(new), Some(old))]
            }
            (None, Some(new)) => {
                let event_type = if new_is_dir {
                    WorkspaceEventType::DirectoryChanged
                } else {
                    WorkspaceEventType::FileCreated
                };
                vec![make_event(workspace_id, event_type, Some(new), None)]
            }
            (Some(old), None) => vec![make_event(
                workspace_id,
                WorkspaceEventType::FileDeleted,
                Some(old),
                None,
            )],
            (None, None) => Vec::new(),
        }
    } else if let Some(path) = event.paths.first() {
        let Some(relative) = relative_path(root, path) else {
            return Vec::new();
        };
        let is_dir = std::fs::metadata(path)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        let event_type = if is_dir {
            WorkspaceEventType::DirectoryChanged
        } else {
            WorkspaceEventType::FileRenamed
        };
        vec![make_event(workspace_id, event_type, Some(relative), None)]
    } else {
        Vec::new()
    }
}

fn make_event(
    workspace_id: &str,
    event_type: WorkspaceEventType,
    path: Option<String>,
    old_path: Option<String>,
) -> WorkspaceEvent {
    let mut payload = serde_json::json!({ "workspace_id": workspace_id });
    if let Some(path) = &path {
        payload["path"] = serde_json::Value::String(path.clone());
    }
    if let Some(old_path) = &old_path {
        payload["old_path"] = serde_json::Value::String(old_path.clone());
    }

    WorkspaceEvent {
        event_id: new_id("evt"),
        workspace_id: workspace_id.to_string(),
        event_type,
        path,
        old_path,
        timestamp: now(),
        payload,
    }
}

/// Convert an absolute path inside the root into a `/`-separated relative path.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    if !path.starts_with(root) {
        return None;
    }
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

fn parent_relative(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::WorkspaceService;
    use fleet_core_protocol::workspace::RegisterWorkspaceRequest;

    async fn wait_for_event(
        receiver: &mut broadcast::Receiver<WorkspaceEvent>,
        expected_path: &str,
    ) -> Option<WorkspaceEvent> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, receiver.recv()).await {
                Ok(Ok(event)) => {
                    let path_matches = event.path.as_deref() == Some(expected_path);
                    let relevant = matches!(
                        event.event_type,
                        WorkspaceEventType::FileCreated | WorkspaceEventType::DirectoryChanged
                    );
                    if path_matches && relevant {
                        return Some(event);
                    }
                }
                Ok(Err(_)) => return None,
                Err(_) => return None,
            }
        }
    }

    #[tokio::test]
    async fn emits_event_when_file_is_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let database = Arc::new(DaemonStorage::connect_in_memory().await.expect("db"));
        let service = WorkspaceService::new(database, WorkspaceSettings::default());
        let workspace = service
            .register(
                "machine_1",
                RegisterWorkspaceRequest {
                    root_path: dir.path().to_string_lossy().into_owned(),
                    name: None,
                    project_id: None,
                },
            )
            .await
            .expect("register");

        let mut watcher = service.watch(&workspace.id).await.expect("watch");
        // Give the OS watch a moment to become active.
        tokio::time::sleep(Duration::from_millis(50)).await;

        std::fs::write(dir.path().join("created.txt"), b"hello").expect("write");
        let mut event = wait_for_event(&mut watcher.events, "created.txt").await;

        if event.is_none() {
            // One retry keeps the test robust on slow or busy CI machines.
            std::fs::write(dir.path().join("created-again.txt"), b"hello").expect("write");
            event = wait_for_event(&mut watcher.events, "created-again.txt").await;
        }

        watcher.stop().await;
        let event = event.expect("workspace event within timeout");
        assert_eq!(event.workspace_id, workspace.id);
        assert!(!event.event_id.is_empty());
    }
}
