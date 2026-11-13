use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_fleet_common::WorkspaceId;
use agent_fleet_protocol::workspace::Workspace;
use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::services::Services;

/// Fan-out for events produced by the daemon (agent, workspace, git).
#[derive(Clone)]
pub struct EventOutbox {
    tx: broadcast::Sender<(String, Value)>,
}

impl Default for EventOutbox {
    fn default() -> Self {
        Self::new()
    }
}

impl EventOutbox {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(2048).0,
        }
    }

    pub fn send(&self, event: impl Into<String>, payload: Value) {
        let _ = self.tx.send((event.into(), payload));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, Value)> {
        self.tx.subscribe()
    }
}

/// Starts and owns workspace filesystem + git-status watchers.
#[derive(Clone)]
pub struct WatcherManager {
    services: Arc<Services>,
    outbox: EventOutbox,
    shutdown: CancellationToken,
    watched: Arc<DashMap<WorkspaceId, ()>>,
    generations: Arc<DashMap<WorkspaceId, u64>>,
    counter: Arc<AtomicU64>,
}

impl WatcherManager {
    pub fn new(services: Arc<Services>, outbox: EventOutbox, shutdown: CancellationToken) -> Self {
        Self {
            services,
            outbox,
            shutdown,
            watched: Arc::new(DashMap::new()),
            generations: Arc::new(DashMap::new()),
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Start watching a workspace exactly once.
    pub async fn ensure(&self, workspace: Workspace) {
        if self.watched.insert(workspace.id.clone(), ()).is_some() {
            return;
        }
        let mut watcher = match self.services.workspaces.watch(&workspace.id).await {
            Ok(watcher) => watcher,
            Err(err) => {
                self.watched.remove(&workspace.id);
                tracing::warn!(workspace = %workspace.id, error = %err, "failed to start workspace watcher");
                return;
            }
        };

        let outbox = self.outbox.clone();
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
                    event = watcher.events.recv() => {
                        match event {
                            Ok(event) => {
                                if let Ok(payload) = serde_json::to_value(&event) {
                                    outbox.send("workspace.event", payload);
                                }
                                if is_git {
                                    schedule_git_refresh(
                                        &generations,
                                        &counter,
                                        git.clone(),
                                        outbox.clone(),
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
}

fn schedule_git_refresh(
    generations: &Arc<DashMap<WorkspaceId, u64>>,
    counter: &Arc<AtomicU64>,
    git: agent_fleet_git_service::GitService,
    outbox: EventOutbox,
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
                outbox.send("git.status.changed", payload);
            }
            Err(err) => {
                tracing::debug!(workspace = %workspace_id, error = %err, "git status refresh failed");
            }
        }
    });
}
