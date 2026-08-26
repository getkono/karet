//! The one delayed-loading model.
//!
//! Every "reserve the surface now, fill it when the backend answers" state in
//! the app anchors on a [`Pending`]: the moment the work began. The shared
//! policy (see `CLAUDE.md` "UI loading states") renders nothing for fast paths
//! and reveals a stable, muted placeholder only once the work has been pending
//! longer than [`LOADING_REVEAL_DELAY`](super::LOADING_REVEAL_DELAY) — and the
//! event loop schedules a repaint at exactly that threshold so the placeholder
//! appears even if no input arrives.

use std::time::Duration;
use std::time::Instant;

use super::App;
use super::LOADING_REVEAL_DELAY;
use super::SidebarPanel;
use super::TabKind;
use super::github::GithubViewState;

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
}

impl App {
    /// Every currently in-flight delayed-loading anchor the UI serves: the
    /// Source-Control sidebar's log/snapshot loads and each open tab's pending
    /// loads. This is the one enumeration behind the event loop's
    /// reveal-deadline wake — a load missing here would show its placeholder
    /// only on the next input event.
    pub(super) fn pendings(&self) -> Vec<Pending> {
        let mut pendings = Vec::new();
        if self.sidebar_visible && self.sidebar_panel == SidebarPanel::SourceControl {
            pendings.extend(self.scm.log_loading_since);
            if self.scm.repository.is_none() {
                pendings.extend(self.scm.repository_loading_since);
            }
        }
        pendings.extend(self.active_outline_loading());
        for tab in self.all_tabs() {
            if let Some(conflict) = tab
                .merge_conflict
                .as_ref()
                .filter(|conflict| conflict.current.is_none() && conflict.error.is_none())
            {
                pendings.push(conflict.loading_since);
            }
            match &tab.kind {
                TabKind::LanguageServers(view) => pendings.extend(view.loading_since),
                // Indexing a repository takes seconds, so the reveal has to be scheduled
                // rather than left to whenever a key happens to arrive.
                TabKind::Seam(state) => pendings.extend(state.loading_since),
                TabKind::CommitLoading {
                    loading_since,
                    error: None,
                    ..
                }
                | TabKind::LatexPreview {
                    loading_since,
                    error: None,
                    ..
                } => pendings.push(*loading_since),
                TabKind::MarkdownPreview { pending_since, .. } => {
                    pendings.extend(*pending_since);
                },
                TabKind::Commit { files, .. } | TabKind::Compare { files, .. } => {
                    pendings.extend(files.loading_since);
                },
                TabKind::Diff {
                    file: None,
                    loading_since,
                    error: None,
                    ..
                } => pendings.extend(*loading_since),
                TabKind::CommitGraph {
                    loading_since,
                    detail_loading_since,
                    files,
                    ..
                } => pendings.extend(
                    [*loading_since, *detail_loading_since, files.loading_since]
                        .into_iter()
                        .flatten(),
                ),
                TabKind::Github(GithubViewState::Dashboard(dashboard)) => {
                    pendings.extend(dashboard.loading_since);
                },
                TabKind::Github(GithubViewState::Issue {
                    pending: Some(_),
                    loading_since,
                    error: None,
                    ..
                }) => pendings.push(*loading_since),
                TabKind::Github(GithubViewState::PullRequest(view))
                    if view.pending.is_some() && view.error.is_none() =>
                {
                    pendings.push(view.loading_since);
                },
                _ => {},
            }
        }
        pendings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_pending_is_hidden_and_wakes_within_the_delay() {
        let pending = Pending::start();
        assert!(!pending.visible());
        let wake = pending.wake(Instant::now());
        assert!(wake.is_some_and(|d| d <= LOADING_REVEAL_DELAY));
    }

    #[test]
    fn a_revealed_pending_is_visible_and_needs_no_wake() {
        let pending = Pending::revealed();
        assert!(pending.visible());
        assert_eq!(pending.wake(Instant::now()), None);
    }
}
