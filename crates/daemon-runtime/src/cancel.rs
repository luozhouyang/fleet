//! Lightweight cancellation flag shared by adapters and the runtime.

use std::sync::Arc;

use tokio::sync::watch;

/// A cloneable cancellation flag with an async notification edge.
#[derive(Clone)]
pub(crate) struct CancelFlag {
    inner: Arc<Inner>,
}

struct Inner {
    tx: watch::Sender<bool>,
}

impl Default for CancelFlag {
    fn default() -> Self {
        let (tx, _rx) = watch::channel(false);
        Self {
            inner: Arc::new(Inner { tx }),
        }
    }
}

impl CancelFlag {
    /// Create a new, un-cancelled flag.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Mark the flag cancelled and wake all waiters.
    pub(crate) fn cancel(&self) {
        self.inner.tx.send_replace(true);
    }

    /// Whether cancellation has been requested.
    pub(crate) fn is_cancelled(&self) -> bool {
        *self.inner.tx.borrow()
    }

    /// Resolve as soon as cancellation is requested.
    pub(crate) async fn cancelled(&self) {
        let mut rx = self.inner.tx.subscribe();
        loop {
            if *rx.borrow() {
                return;
            }
            if rx.changed().await.is_err() {
                // The sender is owned by the flag itself, so this should not
                // happen unless the flag is being dropped.
                return;
            }
        }
    }
}
