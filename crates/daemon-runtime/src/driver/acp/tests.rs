//! Shared test helpers for the ACP driver.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::event::AgentEventEmitter;

/// Build an emitter whose events are not persisted anywhere.
pub(crate) fn test_emitter(session_id: &str) -> AgentEventEmitter {
    let (tx, _) = broadcast::channel(256);
    let (dispatch, _receivers) = crate::event::DispatchQueues::new();
    AgentEventEmitter::new(
        session_id,
        "acp",
        tx,
        dispatch,
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
    )
}
