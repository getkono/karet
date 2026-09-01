//! Notification and diagnostic state updates.
//!
//! Every user-facing message in the app goes through here. There is one renderer
//! — the toast stack — and a call site chooses a [`Report`] tier describing *what
//! kind of thing happened*, never how to draw it. Severity (the colour) and
//! lifetime (how long the card stays) are derived from the tier, so there is
//! exactly one place to tune either.

use super::*;

/// How long a refusal stays before it clears itself.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an outcome stays before it clears itself.
const OUTCOME_TIMEOUT: Duration = Duration::from_secs(3);
/// How long an activity tick stays before it clears itself.
///
/// Longer than an outcome because it is meant to be *refreshed*: a stream of
/// ticks under one tag keeps rewriting the same card, and the card should only
/// fall away once the stream has actually stopped.
const ACTIVITY_TIMEOUT: Duration = Duration::from_secs(8);

/// How a message is surfaced: its severity colour and how long it stays.
///
/// [`Alert`](Self::Alert) and [`Refusal`](Self::Refusal) share a colour
/// deliberately: the colour says *how bad* it is, the lifetime says *whether it
/// needs you*. A refusal the user provoked clears itself; an alert they did not
/// provoke waits to be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Report {
    /// An operation was attempted and failed. Red, persists until dismissed.
    Failure,
    /// A condition the user must not miss, that no action of theirs just caused.
    /// Yellow, persists until dismissed.
    Alert,
    /// A precondition was not met, or there was nothing to do. Yellow, transient.
    Refusal,
    /// The action worked. Teal, transient.
    Outcome,
    /// Something is happening, reported by a source that will never say it
    /// stopped. Blue, transient.
    ///
    /// Distinct from [`App::notify_progress`], which is for an operation the app
    /// *tracks*: that card is persistent and is retired deliberately by the code
    /// that owns the operation. This one is for a stream of ticks nobody owns —
    /// a server's `language/status` chatter — where the only honest end-of-work
    /// signal is the ticks stopping, so the card has to time out on its own.
    Activity,
}

impl Report {
    /// The severity this tier renders as (and logs at).
    pub(crate) fn severity(self) -> Severity {
        match self {
            Self::Failure => Severity::Error,
            Self::Alert | Self::Refusal => Severity::Warning,
            // The hint role reads as success — see `karet_core::severity_role`.
            Self::Outcome => Severity::Hint,
            Self::Activity => Severity::Information,
        }
    }

    /// The tier for a message whose severity a *producer* chose — a config
    /// diagnostic, a backend `Notification` event, an LSP runtime transition.
    ///
    /// A producer-sent warning becomes an [`Alert`](Self::Alert), never a
    /// [`Refusal`](Self::Refusal): nothing the user just did provoked it, so there
    /// is no action of theirs for it to be a refusal *of*, and auto-expiring it
    /// would drop a broken config or a crashed server on the floor.
    ///
    /// `Information` becomes [`Activity`](Self::Activity) rather than
    /// [`Outcome`](Self::Outcome): producers send refusals at that severity ("no
    /// debug session to stop"), and the outcome tier is teal — it would paint a
    /// declined command as one that worked.
    pub(crate) fn from_severity(severity: Severity) -> Self {
        match severity {
            Severity::Error => Self::Failure,
            Severity::Warning => Self::Alert,
            Severity::Hint => Self::Outcome,
            // Information is also the graceful floor for unrecognized severities.
            _ => Self::Activity,
        }
    }

    /// How long a card of this tier lives. `None` means it waits to be dismissed.
    pub(crate) fn timeout(self) -> Option<Duration> {
        match self {
            Self::Failure | Self::Alert => None,
            Self::Refusal => Some(REFUSAL_TIMEOUT),
            Self::Outcome => Some(OUTCOME_TIMEOUT),
            Self::Activity => Some(ACTIVITY_TIMEOUT),
        }
    }
}

impl App {
    /// The notification tag a running source-control operation shares.
    pub(in crate::app) const VCS_OPERATION_TAG: &'static str = "vcs.operation";
    /// The notification tag a running commit shares.
    pub(in crate::app) const VCS_COMMIT_TAG: &'static str = "vcs.commit";
    /// The notification tag a seam index/sync shares.
    pub(in crate::app) const SEAM_SYNC_TAG: &'static str = "seam.sync";
    /// The notification tag language-server status chatter shares.
    pub(in crate::app) const LSP_STATUS_TAG: &'static str = "lsp.status";
    /// The notification tag a dependency check shares.
    pub(in crate::app) const DEPS_CHECK_TAG: &'static str = "deps.check";
    /// The notification tag a save-many-before-X batch shares.
    pub(in crate::app) const SAVE_BATCH_TAG: &'static str = "save.batch";
    /// The notification tag a diff computation shares.
    pub(in crate::app) const DIFF_COMPUTE_TAG: &'static str = "diff.compute";
    /// The notification tag the debug session's own state changes share.
    pub(in crate::app) const DEBUG_SESSION_TAG: &'static str = "debug.session";
    /// The notification tag a running notebook kernel shares.
    pub(in crate::app) const NOTEBOOK_KERNEL_TAG: &'static str = "notebook.kernel";
    /// The notification tag a notebook kernel's failures share.
    ///
    /// Separate from [`Self::NOTEBOOK_KERNEL_TAG`] so the two never displace each
    /// other: progress must not bury a failure, and a failure must not be erased
    /// by the next tick. Sharing one tag among failures still collapses a cell
    /// re-run that keeps raising, which would otherwise stack a permanent card
    /// per attempt and push the older ones off the visible stack.
    pub(in crate::app) const NOTEBOOK_KERNEL_FAILURE_TAG: &'static str = "notebook.kernel.failure";
    /// The notification tag a document save reports under.
    pub(in crate::app) const SAVED_TAG: &'static str = "io.saved";
    /// The notification tag a clipboard copy reports under.
    pub(in crate::app) const CLIPBOARD_TAG: &'static str = "io.clipboard";
    /// The notification tag a working-tree discard shares.
    pub(in crate::app) const VCS_DISCARD_TAG: &'static str = "vcs.discard";
    /// The notification tag a repository-facts lookup shares.
    pub(in crate::app) const REMOTE_FACTS_TAG: &'static str = "vcs.remote-facts";
    /// The notification tag a pull-request listing shares.
    pub(in crate::app) const PULL_REQUESTS_TAG: &'static str = "vcs.pull-requests";

    /// Push a notification onto the center at the given [`Report`] tier.
    pub(super) fn notify(
        &mut self,
        tier: Report,
        kind: NotificationKind,
        title: impl Into<String>,
    ) {
        self.notify_tagged(tier, kind, title, None);
    }

    /// Push a notification carrying an explanatory second line.
    ///
    /// For the case where one event has both a summary and a reason: the toast
    /// renders the body under the title, which keeps them on one card instead of
    /// making the reader dismiss two.
    pub(super) fn notify_detailed(
        &mut self,
        tier: Report,
        kind: NotificationKind,
        title: impl Into<String>,
        body: String,
    ) {
        self.push_report(tier, kind, title.into(), Some(body), None);
    }

    /// Push a notification, optionally replacing whatever holds `tag`.
    ///
    /// The tagged form is how an outcome supersedes its own progress card: both
    /// carry the same tag, so the card is rewritten in place instead of stacking a
    /// second one beneath it.
    pub(super) fn notify_tagged(
        &mut self,
        tier: Report,
        kind: NotificationKind,
        title: impl Into<String>,
        tag: Option<String>,
    ) {
        self.push_report(tier, kind, title.into(), None, tag);
    }

    /// The one place a [`Report`] becomes a notification.
    fn push_report(
        &mut self,
        tier: Report,
        kind: NotificationKind,
        title: String,
        body: Option<String>,
        tag: Option<String>,
    ) {
        match tier {
            Report::Failure => {
                tracing::error!(notification_kind = ?kind, message = %title, "notification");
            },
            Report::Alert | Report::Refusal => {
                tracing::warn!(notification_kind = ?kind, message = %title, "notification");
            },
            Report::Outcome | Report::Activity => {},
        }
        self.notifications.push(
            Notification {
                id: NotificationId(0),
                severity: tier.severity(),
                kind,
                title,
                body,
                tag,
                timeout: tier.timeout(),
                dismissable: true,
            },
            Instant::now(),
        );
    }

    /// Push or replace the card for a running operation.
    ///
    /// Persistent (no timeout) because the work is still going: an auto-expiring
    /// card would vanish mid-download and leave the user with no sign anything is
    /// happening. `NotificationCenter::push` replaces any active card sharing this
    /// `tag`, so progress updates in place rather than stacking, and the eventual
    /// success or failure supersedes it under the same tag.
    pub(super) fn notify_progress(
        &mut self,
        kind: NotificationKind,
        tag: String,
        title: impl Into<String>,
        body: Option<String>,
    ) {
        self.notifications.push(
            Notification {
                id: NotificationId(0),
                severity: Severity::Information,
                kind,
                title: title.into(),
                body,
                tag: Some(tag),
                timeout: None,
                // Dismissable: the user may not care about a background download,
                // and the manager tab still has the detail.
                dismissable: true,
            },
            Instant::now(),
        );
    }

    /// Surface a dropped backend-submission error as a persistent notification, so a
    /// closed or wedged backend never fails silently.
    pub(super) fn notify_backend_error(&mut self, error: BackendError) {
        self.notify(
            Report::Failure,
            NotificationKind::System,
            format!("backend: {error}"),
        );
    }

    /// Replace the complete merged diagnostic set for one document.
    pub(super) fn replace_document_diagnostics(
        &mut self,
        doc: DocumentId,
        diagnostics: Vec<Diagnostic>,
    ) {
        if diagnostics.is_empty() {
            self.docs.diagnostics.remove(&doc);
        } else {
            self.docs.diagnostics.insert(doc, diagnostics);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_and_alerts_persist_while_refusals_and_outcomes_expire() {
        // The whole point of the tier split: a message the user provoked clears
        // itself, one they did not waits to be read.
        assert_eq!(Report::Failure.timeout(), None);
        assert_eq!(Report::Alert.timeout(), None);
        assert_eq!(Report::Refusal.timeout(), Some(REFUSAL_TIMEOUT));
        assert_eq!(Report::Outcome.timeout(), Some(OUTCOME_TIMEOUT));
    }

    #[test]
    fn every_tier_maps_to_its_rendering_severity() {
        assert_eq!(Report::Failure.severity(), Severity::Error);
        assert_eq!(Report::Alert.severity(), Severity::Warning);
        assert_eq!(Report::Refusal.severity(), Severity::Warning);
        assert_eq!(Report::Outcome.severity(), Severity::Hint);
    }

    #[test]
    fn no_tier_renders_as_the_status_bar_it_replaced() {
        // The defect this change closes: the old status line drew every message in
        // the status bar's own style, so an error was exactly the colour of the
        // hints it covered. Every tier now carries a severity the toast colours by.
        for tier in [
            Report::Failure,
            Report::Alert,
            Report::Refusal,
            Report::Outcome,
        ] {
            assert_ne!(
                karet_core::severity_role(tier.severity()),
                karet_core::ThemeRole::StatusBarForeground
            );
        }
    }
}
