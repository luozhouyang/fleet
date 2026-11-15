use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use fleet_core_common::WorkspaceId;
use fleet_core_protocol::workspace::Workspace;
use fleet_daemon_workspace::WorkspaceWatcherStop;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::services::Services;

/// Fan-out for events produced by the daemon (agent, workspace, git).
#[derive(Clone)]
pub struct DaemonEventFanout {
    tx: broadcast::Sender<(String, Value, Option<u64>)>,
}

impl Default for DaemonEventFanout {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonEventFanout {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(2048).0,
        }
    }

    pub fn send(&self, event: impl Into<String>, payload: Value) {
        let _ = self.tx.send((event.into(), payload, None));
    }

    /// Queue a durable agent event with the contiguous delivery watermark the
    /// admin may acknowledge.
    pub fn send_with_cursor(&self, event: impl Into<String>, payload: Value, cursor: u64) {
        let _ = self.tx.send((event.into(), payload, Some(cursor)));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, Value, Option<u64>)> {
        self.tx.subscribe()
    }
}

/// Starts and owns workspace filesystem + git-status watchers.
#[derive(Clone)]
pub struct WatcherManager {
    services: Arc<Services>,
    fanout: DaemonEventFanout,
    shutdown: CancellationToken,
    watched: Arc<DashMap<WorkspaceId, Arc<tokio::sync::Mutex<Option<WorkspaceWatcherStop>>>>>,
    generations: Arc<DashMap<WorkspaceId, u64>>,
    counter: Arc<AtomicU64>,
}

impl WatcherManager {
    pub fn new(
        services: Arc<Services>,
        fanout: DaemonEventFanout,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            services,
            fanout,
            shutdown,
            watched: Arc::new(DashMap::new()),
            generations: Arc::new(DashMap::new()),
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Start watching a workspace exactly once.
    pub async fn ensure(&self, workspace: Workspace) {
        if self.watched.contains_key(&workspace.id) {
            return;
        }
        let watcher = match self.services.workspaces.watch(&workspace.id).await {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!(workspace = %workspace.id, error = %err, "failed to start workspace watcher");
                return;
            }
        };
        let (mut watcher_events, stop) = watcher.into_parts();
        // A concurrent ensure may have won; dropping the new stop handle ends
        // the duplicate watcher.
        if self
            .watched
            .insert(
                workspace.id.clone(),
                Arc::new(tokio::sync::Mutex::new(Some(stop))),
            )
            .is_some()
        {
            return;
        }

        let fanout = self.fanout.clone();
        let git = self.services.git.clone();
        let shutdown = self.shutdown.clone();
        let generations = self.generations.clone();
        let counter = self.counter.clone();
        let id = workspace.id.clone();
        let root = workspace.root_path.clone();
        let is_git = workspace.is_git_repository;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = watcher_events.recv() => {
                        match event {
                            Ok(event) => {
                                if let Ok(payload) = serde_json::to_value(&event) {
                                    fanout.send("workspace.event", payload);
                                }
                                if is_git {
                                    schedule_git_refresh(
                                        &generations,
                                        &counter,
                                        git.clone(),
                                        fanout.clone(),
                                        id.clone(),
                                        root.clone(),
                                    );
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(workspace = %id, skipped, "workspace watcher lagged");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
            tracing::debug!(workspace = %id, "workspace watcher stopped");
        });
    }

    /// Stop and forget the watcher for a removed workspace.
    pub async fn remove(&self, workspace_id: &str) {
        self.generations.remove(workspace_id);
        if let Some((_, stop)) = self.watched.remove(workspace_id) {
            let stop = stop.lock().await.take();
            if let Some(stop) = stop {
                stop.stop().await;
            }
        }
    }
}

fn schedule_git_refresh(
    generations: &Arc<DashMap<WorkspaceId, u64>>,
    counter: &Arc<AtomicU64>,
    git: fleet_daemon_git::GitService,
    fanout: DaemonEventFanout,
    workspace_id: WorkspaceId,
    root: String,
) {
    let generation = counter.fetch_add(1, Ordering::Relaxed) + 1;
    generations.insert(workspace_id.clone(), generation);
    let generations = generations.clone();
    tokio::spawn(async move {
        // Coalesce bursts of filesystem events into one git status query.
        tokio::time::sleep(Duration::from_millis(400)).await;
        if generations.get(&workspace_id).map(|g| *g) != Some(generation) {
            return;
        }
        match git.status(Path::new(&root)).await {
            Ok(status) => {
                let payload = serde_json::json!({
                    "workspace_id": workspace_id,
                    "status": status,
                });
                fanout.send("git.status.changed", payload);
            }
            Err(err) => {
                tracing::debug!(workspace = %workspace_id, error = %err, "git status refresh failed");
            }
        }
    });
}
