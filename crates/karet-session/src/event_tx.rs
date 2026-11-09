//! The sending half of the client event stream: the one type the session and
//! every worker it starts answer through.
//!
//! Each [`Event`] travels with the [`RequestId`] it answers, or `None` when it
//! is unsolicited. A send can only fail because the client dropped its
//! [`EventRx`](crate::session::EventRx): nobody is left to tell, so [`emit`]
//! drops the event, and a worker whose loop should stop with the client asks
//! [`try_emit`] instead.
//!
//! [`emit`]: EventTx::emit
//! [`try_emit`]: EventTx::try_emit

use std::ops::ControlFlow;

use karet_core::NotificationKind;
use karet_core::Severity;
use tokio::sync::mpsc;

use crate::api::Event;
use crate::api::RequestId;
use crate::cancellation::Cancellation;

/// A cloneable handle onto the client event stream.
#[derive(Clone, Debug)]
pub(crate) struct EventTx(mpsc::UnboundedSender<(Option<RequestId>, Event)>);

impl EventTx {
    /// A new event channel: this sending half, and the raw receiving half
    /// the session wraps as its [`EventRx`](crate::session::EventRx).
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<(Option<RequestId>, Event)>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }

    /// Send `event`, answering `id`. Dropped when the client is gone.
    pub(crate) fn emit(&self, id: Option<RequestId>, event: Event) {
        let _ = self.0.send((id, event));
    }

    /// Send `event`, answering `id`, and say whether anyone is still
    /// listening: [`ControlFlow::Break`] once the client is gone, so a worker
    /// loop can stop producing for nobody.
    pub(crate) fn try_emit(&self, id: Option<RequestId>, event: Event) -> ControlFlow<()> {
        match self.0.send((id, event)) {
            Ok(()) => ControlFlow::Continue(()),
            Err(_) => ControlFlow::Break(()),
        }
    }

    /// Send `event`, answering `id`, unless `cancel` says the client has moved
    /// on: a late answer would contradict the state it is showing.
    pub(crate) fn emit_if_live(&self, id: Option<RequestId>, cancel: &Cancellation, event: Event) {
        if !cancel.is_cancelled() {
            self.emit(id, event);
        }
    }

    /// Send a user-facing [`Event::Notification`], answering `id`.
    pub(crate) fn notify(
        &self,
        id: Option<RequestId>,
        severity: Severity,
        kind: NotificationKind,
        message: impl Into<String>,
    ) {
        self.emit(
            id,
            Event::Notification {
                severity,
                kind,
                message: message.into(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_tags_the_event_with_its_request() {
        let (events, mut rx) = EventTx::channel();
        events.emit(Some(RequestId(7)), Event::CommitCancelled);
        assert!(matches!(
            rx.try_recv(),
            Ok((Some(RequestId(7)), Event::CommitCancelled))
        ));
    }

    #[test]
    fn try_emit_breaks_once_the_client_is_gone() {
        let (events, rx) = EventTx::channel();
        assert_eq!(
            events.try_emit(None, Event::CommitCancelled),
            ControlFlow::Continue(())
        );
        drop(rx);
        assert_eq!(
            events.try_emit(None, Event::CommitCancelled),
            ControlFlow::Break(())
        );
        // A plain emit to a departed client is dropped, not an error.
        events.emit(None, Event::CommitCancelled);
    }

    #[test]
    fn emit_if_live_drops_a_cancelled_answer() {
        let (events, mut rx) = EventTx::channel();
        let hub = crate::cancellation::CancellationHub::default();
        let cancel = hub.register(RequestId(1));
        events.emit_if_live(Some(RequestId(1)), &cancel, Event::CommitCancelled);
        hub.cancel(RequestId(1));
        events.emit_if_live(Some(RequestId(2)), &cancel, Event::CommitCancelled);
        assert!(matches!(rx.try_recv(), Ok((Some(RequestId(1)), _))));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn notify_sends_a_notification() {
        let (events, mut rx) = EventTx::channel();
        events.notify(
            None,
            Severity::Error,
            NotificationKind::Vcs,
            "no repository",
        );
        assert!(matches!(
            rx.try_recv(),
            Ok((None, Event::Notification { severity: Severity::Error, kind: NotificationKind::Vcs, message }))
                if message == "no repository"
        ));
    }
}
