//! The one delayed-loading model.
//!
//! Every "reserve the surface now, fill it when the backend answers" state in
//! the app anchors on a [`Pending`]: the moment the work began. The shared
//! policy (see `CLAUDE.md` "UI loading states") renders nothing for fast paths
//! and reveals a stable, muted placeholder only once the work has been pending
//! longer than [`LOADING_REVEAL_DELAY`] — and the event loop schedules a
//! repaint at exactly that threshold so the placeholder appears even if no
//! input arrives.
//!
//! A root module rather than part of `app`, because the models the app holds
//! (tabs, the workspace index, preview images) carry their waits themselves and
//! must not depend on the app to say so.

use std::time::Duration;
use std::time::Instant;

/// Delay before rendering non-blocking loading text. Fast operations can complete
/// without visual churn; slower ones get an explicit, stable placeholder.
pub(crate) const LOADING_REVEAL_DELAY: Duration = Duration::from_millis(200);

/// The start of an in-flight load, carrying the shared reveal policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pending {
    since: Instant,
}

impl Pending {
    /// Anchor a load starting now.
    #[must_use]
    pub(crate) fn start() -> Self {
        Self {
            since: Instant::now(),
        }
    }

    /// Whether the loading placeholder should be visible: the work has been
    /// pending beyond the shared reveal delay.
    #[must_use]
    pub(crate) fn visible(self) -> bool {
        self.since.elapsed() >= LOADING_REVEAL_DELAY
    }

    /// Time from `now` until this load's placeholder must be revealed, or
    /// `None` when it is already visible (no wake needed — it is painted).
    #[must_use]
    pub(crate) fn wake(self, now: Instant) -> Option<Duration> {
        LOADING_REVEAL_DELAY.checked_sub(now.saturating_duration_since(self.since))
    }

    /// How long this load has been pending as of `now` (drives spinner phases).
    #[must_use]
    pub(crate) fn elapsed_since(self, now: Instant) -> Duration {
        now.saturating_duration_since(self.since)
    }

    /// A pending state backdated past the reveal delay, so tests can assert on
    /// the visible placeholder without sleeping.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn revealed() -> Self {
        Self {
            since: Instant::now() - LOADING_REVEAL_DELAY,
        }
    }

    /// A pending state anchored at an exact instant, for timing-precise tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn at(since: Instant) -> Self {
        Self { since }
    }

    /// The instant this load is anchored at, so a test can ask [`Self::wake`]
    /// about fixed offsets from it instead of racing the real clock.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn since(self) -> Instant {
        self.since
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_pending_is_hidden() {
        assert!(!Pending::start().visible());
    }

    #[test]
    fn the_wake_counts_down_exactly_to_the_reveal_delay() {
        let t0 = Instant::now();
        let pending = Pending::at(t0);
        let step = Duration::from_millis(50);
        assert_eq!(
            pending.wake(t0),
            Some(LOADING_REVEAL_DELAY),
            "a full delay at the anchor"
        );
        assert_eq!(pending.wake(t0 + step), Some(LOADING_REVEAL_DELAY - step));
        assert_eq!(
            pending.wake(t0 + LOADING_REVEAL_DELAY + Duration::from_millis(1)),
            None,
            "past the delay the placeholder is painted, so nothing wakes"
        );
        assert_eq!(pending.elapsed_since(t0 + step), step);
    }

    #[test]
    fn an_instant_before_the_anchor_saturates_to_a_full_delay() {
        let t0 = Instant::now();
        let pending = Pending::at(t0 + Duration::from_secs(1));
        assert_eq!(pending.wake(t0), Some(LOADING_REVEAL_DELAY));
        assert_eq!(pending.elapsed_since(t0), Duration::ZERO);
    }

    #[test]
    fn a_revealed_pending_is_visible_and_needs_no_wake() {
        let pending = Pending::revealed();
        assert!(pending.visible());
        assert_eq!(pending.wake(Instant::now()), None);
    }
}
