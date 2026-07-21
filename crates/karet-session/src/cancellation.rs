//! Cooperative cancellation shared by serialized background workers.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use crate::api::RequestId;

/// Registry connecting actor-side cancellation commands to worker-side requests.
#[derive(Clone, Default)]
pub(crate) struct CancellationHub {
    tasks: Arc<Mutex<HashMap<RequestId, Arc<AtomicBool>>>>,
}

impl CancellationHub {
    /// Register one safely-droppable request before enqueueing it.
    pub(crate) fn register(&self, id: RequestId) -> Cancellation {
        let cancelled = Arc::new(AtomicBool::new(false));
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.insert(id, cancelled.clone());
        }
        Cancellation {
            id,
            cancelled,
            tasks: Arc::downgrade(&self.tasks),
        }
    }

    /// Ask a registered request to stop at its next cancellation point.
    pub(crate) fn cancel(&self, id: RequestId) {
        if let Ok(tasks) = self.tasks.lock()
            && let Some(cancelled) = tasks.get(&id)
        {
            cancelled.store(true, Ordering::Release);
        }
    }
}

/// Cooperative cancellation token owned by one worker job.
pub(crate) struct Cancellation {
    id: RequestId,
    cancelled: Arc<AtomicBool>,
    tasks: Weak<Mutex<HashMap<RequestId, Arc<AtomicBool>>>>,
}

impl Cancellation {
    /// Whether the request has been cancelled by its owner.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
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
