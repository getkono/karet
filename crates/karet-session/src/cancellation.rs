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
    /// Run once, when the request is cancelled.
    ///
    /// A flag only works for a job that comes back to look at it. A job blocked
    /// inside an external process never does, so it registers what cancelling it
    /// actually means — for a commit, signalling its process group.
    hook: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
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
        // Take the state out of the registry lock before running anything: a
        // hook is arbitrary caller code, and running it under the lock every
        // other request contends on invites a deadlock for no benefit.
        let state = self
            .tasks
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(&id).cloned());
        let Some(state) = state else {
            return;
        };
        state.cancelled.store(true, Ordering::Release);
        // There is one waiter per token. `notify_one` retains a permit when
        // cancellation races with the waiter starting to poll.
        state.notify.notify_one();
        state.fire();
    }
}

/// Cooperative cancellation token owned by one worker job.
pub(crate) struct Cancellation {
    id: RequestId,
    state: Arc<CancellationState>,
    tasks: Weak<Mutex<HashMap<RequestId, Arc<CancellationState>>>>,
}

impl CancellationState {
    /// Run the registered hook, once.
    fn fire(&self) {
        let hook = self.hook.lock().ok().and_then(|mut hook| hook.take());
        if let Some(hook) = hook {
            hook();
        }
    }
}

impl Cancellation {
    /// Register what cancelling this request should *do*, beyond setting a flag.
    ///
    /// Runs immediately when the request has already been cancelled, which is
    /// what keeps a cancellation that arrives while the job is still queued from
    /// being lost.
    pub(crate) fn on_cancel(&self, action: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut hook) = self.state.hook.lock() {
            *hook = Some(Box::new(action));
        }
        if self.is_cancelled() {
            self.state.fire();
        }
    }

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
        // Remove only this token's own registration: when the id was registered
        // again, the entry belongs to the newer token, and removing it would leave
        // that request uncancellable.
        if let Some(tasks) = self.tasks.upgrade()
            && let Ok(mut tasks) = tasks.lock()
            && tasks
                .get(&self.id)
                .is_some_and(|state| Arc::ptr_eq(state, &self.state))
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

    #[tokio::test(start_paused = true)]
    async fn an_uncancelled_token_keeps_its_reader_waiting() {
        let hub = CancellationHub::default();
        let token = hub.register(RequestId(10));
        // The paused clock jumps straight to the deadline once nothing else can run.
        let waited =
            tokio::time::timeout(std::time::Duration::from_secs(60), token.cancelled()).await;
        assert!(
            waited.is_err(),
            "nothing cancelled it, so it must not return"
        );
    }

    /// A counter and a hook that bumps it.
    fn counting_hook() -> (
        Arc<std::sync::atomic::AtomicUsize>,
        impl Fn() + Send + Sync + 'static,
    ) {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook_count = count.clone();
        (count, move || {
            hook_count.fetch_add(1, Ordering::SeqCst);
        })
    }

    #[test]
    fn on_cancel_runs_at_once_for_a_request_cancelled_while_queued() {
        let hub = CancellationHub::default();
        let token = hub.register(RequestId(1));
        hub.cancel(RequestId(1));
        let (count, hook) = counting_hook();
        token.on_cancel(hook);
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "a queued cancel is not lost"
        );
        assert!(token.is_cancelled());
    }

    #[test]
    fn the_cancel_hook_runs_at_most_once() {
        let hub = CancellationHub::default();
        let token = hub.register(RequestId(2));
        let (count, hook) = counting_hook();
        token.on_cancel(hook);
        assert_eq!(count.load(Ordering::SeqCst), 0, "registering does not fire");
        hub.cancel(RequestId(2));
        hub.cancel(RequestId(2));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "a repeated cancel fires nothing"
        );
    }

    #[test]
    fn dropping_a_token_unregisters_its_request() {
        let hub = CancellationHub::default();
        let token = hub.register(RequestId(3));
        let (count, hook) = counting_hook();
        token.on_cancel(hook);
        drop(token);
        assert!(
            hub.tasks.lock().is_ok_and(|tasks| tasks.is_empty()),
            "a finished request must not stay in the registry"
        );
        hub.cancel(RequestId(3));
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "a dropped request cannot be cancelled"
        );
    }

    #[test]
    fn dropping_an_older_token_keeps_a_newer_registration_of_the_same_id() {
        let hub = CancellationHub::default();
        let older = hub.register(RequestId(4));
        let newer = hub.register(RequestId(4));
        drop(older);
        hub.cancel(RequestId(4));
        assert!(
            newer.is_cancelled(),
            "the newer request must stay cancellable"
        );
        drop(newer);
        assert!(hub.tasks.lock().is_ok_and(|tasks| tasks.is_empty()));
    }
}
