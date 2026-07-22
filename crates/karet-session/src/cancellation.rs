//! Cooperative cancellation shared by serialized background workers.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::sync::Notify;

use crate::api::RequestId;

/// Registry connecting actor-side cancellation commands to worker-side requests.
#[derive(Clone, Default)]
pub(crate) struct CancellationHub {
    tasks: Arc<Mutex<HashMap<RequestId, Arc<CancellationState>>>>,
}

#[derive(Default)]
struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancellationHub {
    /// Register one safely-droppable request before enqueueing it.
    pub(crate) fn register(&self, id: RequestId) -> Cancellation {
        let state = Arc::new(CancellationState::default());
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.insert(id, state.clone());
        }
        Cancellation {
            id,
            state,
            tasks: Arc::downgrade(&self.tasks),
        }
    }

    /// Ask a registered request to stop at its next cancellation point.
    pub(crate) fn cancel(&self, id: RequestId) {
        if let Ok(tasks) = self.tasks.lock()
            && let Some(state) = tasks.get(&id)
        {
            state.cancelled.store(true, Ordering::Release);
            // There is one waiter per token. `notify_one` retains a permit when
            // cancellation races with the waiter starting to poll.
            state.notify.notify_one();
        }
    }
}

/// Cooperative cancellation token owned by one worker job.
pub(crate) struct Cancellation {
    id: RequestId,
    state: Arc<CancellationState>,
    tasks: Weak<Mutex<HashMap<RequestId, Arc<CancellationState>>>>,
}

impl Cancellation {
    /// Whether the request has been cancelled by its owner.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    /// Wait until the owning client cancels this request.
    pub(crate) async fn cancelled(&self) {
        while !self.is_cancelled() {
            self.state.notify.notified().await;
        }
    }
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        if let Some(tasks) = self.tasks.upgrade()
            && let Ok(mut tasks) = tasks.lock()
        {
            tasks.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_wakes_an_async_reader() {
        let hub = CancellationHub::default();
        let token = hub.register(RequestId(9));
        let waiter = tokio::spawn(async move {
            token.cancelled().await;
        });
        tokio::task::yield_now().await;
        hub.cancel(RequestId(9));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .is_ok(),
            "cancellation must wake a pending network-read future"
        );
    }
}
