//! The notification center: the app-side owner of notification lifetime.
//!
//! [`karet_core::Notification`] is clock-free; this center stamps each accepted
//! notification with the [`Instant`] it arrived so it can auto-expire transient ones
//! while keeping persistent errors until the user dismisses them. Expiry is pure with
//! respect to an injected "now", so the event loop can drive it from a timer and the
//! logic stays unit-testable.

use std::time::Duration;
use std::time::Instant;

use karet_core::Notification;
use karet_core::NotificationId;

/// A live notification plus the instant it was accepted.
struct Active {
    note: Notification,
    created: Instant,
}

/// Owns the active notification stack (newest last).
#[derive(Default)]
pub(crate) struct NotificationCenter {
    next_id: u64,
    active: Vec<Active>,
}

impl NotificationCenter {
    /// The most notifications shown at once (matches the widget's cap).
    const MAX_ACTIVE: usize = 5;

    /// Accept `note`, assigning it a fresh [`NotificationId`], and return that id.
    ///
    /// A notification carrying a `tag` replaces any active notification with the same
    /// tag (so a repeated condition updates in place instead of stacking). When the
    /// active stack is full, the oldest *transient* notification is evicted — a
    /// persistent error is never dropped to make room for a shorter-lived one.
    pub(crate) fn push(&mut self, mut note: Notification, now: Instant) -> NotificationId {
        let id = NotificationId(self.next_id);
        self.next_id += 1;
        note.id = id;

        if let Some(tag) = note.tag.clone() {
            self.active
                .retain(|a| a.note.tag.as_deref() != Some(tag.as_str()));
        }

        if self.active.len() >= Self::MAX_ACTIVE
            && let Some(pos) = self.active.iter().position(|a| a.note.timeout.is_some())
        {
            self.active.remove(pos);
        }
        self.active.push(Active { note, created: now });
        id
    }

    /// Dismiss the notification with `id`, if present.
    pub(crate) fn dismiss(&mut self, id: NotificationId) {
        self.active.retain(|a| a.note.id != id);
    }

    /// Dismiss the newest notification, if any.
    pub(crate) fn dismiss_latest(&mut self) {
        self.active.pop();
    }

    /// Dismiss every dismissable notification.
    pub(crate) fn dismiss_all(&mut self) {
        self.active.retain(|a| !a.note.dismissable);
    }

    /// Drop notifications whose relative `timeout` has elapsed as of `now`. Returns
    /// whether anything was removed (a repaint gate).
    pub(crate) fn expire(&mut self, now: Instant) -> bool {
        let before = self.active.len();
        self.active.retain(|a| match a.note.timeout {
            Some(t) => now.duration_since(a.created) < t,
            None => true,
        });
        self.active.len() != before
    }

    /// Whether there is nothing to show.
    pub(crate) fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// The active notifications, newest first (render order).
    pub(crate) fn active(&self) -> Vec<&Notification> {
        self.active.iter().rev().map(|a| &a.note).collect()
    }

    /// The soonest instant at which [`expire`](Self::expire) would remove something,
    /// expressed as a delay from `now`. `None` when nothing is pending (the event
    /// loop then parks on its other sources rather than ticking).
    pub(crate) fn next_deadline(&self, now: Instant) -> Option<Duration> {
        self.active
            .iter()
            .filter_map(|a| {
                a.note
                    .timeout
                    .map(|t| t.saturating_sub(now.duration_since(a.created)))
            })
            .min()
    }
}

#[cfg(test)]
mod tests {
    use karet_core::NotificationKind;
    use karet_core::Severity;

    use super::*;

    fn note(tag: Option<&str>, timeout: Option<Duration>) -> Notification {
        Notification {
            id: NotificationId(0),
            severity: Severity::Error,
            kind: NotificationKind::System,
            title: "t".to_string(),
            body: None,
            tag: tag.map(str::to_string),
            timeout,
            dismissable: true,
        }
    }

    #[test]
    fn ids_increase_monotonically() {
        let mut c = NotificationCenter::default();
        let now = Instant::now();
        let a = c.push(note(None, None), now);
        let b = c.push(note(None, None), now);
        assert_eq!(a, NotificationId(0));
        assert_eq!(b, NotificationId(1));
    }

    #[test]
    fn expire_drops_elapsed_transient_keeps_persistent() {
        let mut c = NotificationCenter::default();
        let start = Instant::now();
        c.push(note(None, Some(Duration::from_secs(1))), start);
        c.push(note(None, None), start); // persistent
        // Before the timeout, nothing expires.
        assert!(!c.expire(start));
        assert_eq!(c.active().len(), 2);
        // After it, only the transient one goes.
        let later = start + Duration::from_secs(2);
        assert!(c.expire(later));
        assert_eq!(c.active().len(), 1);
        assert!(c.active()[0].timeout.is_none());
    }

    #[test]
    fn tag_replaces_in_place() {
        let mut c = NotificationCenter::default();
        let now = Instant::now();
        c.push(note(Some("vcs.status"), None), now);
        c.push(note(Some("vcs.status"), None), now);
        assert_eq!(c.active().len(), 1);
    }

    #[test]
    fn full_stack_never_evicts_a_persistent_error_for_a_transient() {
        let mut c = NotificationCenter::default();
        let now = Instant::now();
        // Fill with persistent notifications.
        for _ in 0..NotificationCenter::MAX_ACTIVE {
            c.push(note(None, None), now);
        }
        // A transient arrival cannot displace a persistent; the stack grows past cap
        // only transiently and no persistent is lost.
        c.push(note(None, Some(Duration::from_secs(1))), now);
        assert_eq!(
            c.active().iter().filter(|n| n.timeout.is_none()).count(),
            NotificationCenter::MAX_ACTIVE
        );
    }

    #[test]
    fn dismiss_and_dismiss_all() {
        let mut c = NotificationCenter::default();
        let now = Instant::now();
        let id = c.push(note(None, None), now);
        c.push(note(None, None), now);
        c.dismiss(id);
        assert_eq!(c.active().len(), 1);
        c.dismiss_all();
        assert!(c.is_empty());
    }

    #[test]
    fn next_deadline_is_soonest_and_none_when_idle() {
        let mut c = NotificationCenter::default();
        let now = Instant::now();
        assert_eq!(c.next_deadline(now), None);
        c.push(note(None, Some(Duration::from_secs(5))), now);
        c.push(note(None, Some(Duration::from_secs(2))), now);
        c.push(note(None, None), now);
        assert_eq!(c.next_deadline(now), Some(Duration::from_secs(2)));
    }
}
