//! Shared test helpers for the ACP driver.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc};

use crate::event::AgentEventEmitter;

/// Build an emitter whose events are not persisted anywhere.
pub(crate) fn test_emitter(session_id: &str) -> AgentEventEmitter {
    let (tx, _) = broadcast::channel(256);
    let (dispatch, _dispatch_rx) = mpsc::unbounded_channel();
    AgentEventEmitter::new(
        session_id,
        "acp",
        tx,
        dispatch,
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
    )
}
