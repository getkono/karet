//! What the event loop waits on: every delayed-loading anchor the app holds
//! ([`App::pendings`]), and the earliest moment something on screen must change
//! without input arriving ([`App::next_wake`]).

use std::time::Duration;
use std::time::Instant;

use karet_widgets::Spinner;

use super::App;
use super::COMMIT_REVEAL;
use super::SidebarPanel;
use crate::pending::Pending;
use crate::tab::TabKind;

impl App {
    /// The soonest the event loop should wake for time-based UI: notification expiry,
    /// save-spinner animation, graphical-caret blink, delayed loading states, or an
    /// expiring hover reveal.
    /// `None` when the loop can park on its event sources alone.
    pub(super) fn next_wake(&self) -> Option<Duration> {
        let now = Instant::now();
        let notif = self.notifications.next_deadline(now);
        let spinner = (!self.pending_saves.is_empty()).then_some(Spinner::FRAME_INTERVAL);
        let auto_save = self
            .auto_save_pending
            .iter()
            .filter(|(doc, _)| !self.pending_saves.values().any(|save| save.doc == **doc))
            .filter_map(|(_, pending)| pending.deadline)
            .map(|deadline| deadline.saturating_duration_since(now))
            .min();
        let caret = self.graphics_caret_next_wake(now);
        let loading = self
            .pendings()
            .into_iter()
            .filter_map(|pending| pending.wake(now))
            .min();
        let nested_repositories = self.nested_repository_next_wake(now);
        // An edited document's hint request goes out once typing pauses, and
        // has to without another keystroke to wake the loop.
        let inlay = self.inlay_next_wake(now);
        let code_lens = self.code_lens_next_wake(now);
        let links_colors = self.link_color_next_wake(now);
        // Likewise the lightbulb probe, once the caret has settled.
        let code_actions = self.code_action_next_wake(now);
        // And the ghost-text ask, once typing has paused.
        let inline_completion = self.inline_completion_next_wake(now);
        // And the occurrence highlight, once the caret has rested on a word.
        let occurrences = self.occurrence_next_wake(now);
        // Likewise a workspace-symbol query, once typing pauses.
        let workspace_symbols = self.workspace_symbol_next_wake(now);
        // Generation runs for seconds with no output to stream, so its spinner is
        // the only progress there is — it has to keep animating without input.
        let ai_commit = self.ai_commit_next_wake(now);
        let operation = self
            .operation_blocker
            .as_ref()
            .map(|blocker| blocker.deadline.saturating_duration_since(now));
        // Wake to repaint (hiding the tooltip) when the commit-badge reveal expires.
        let reveal = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Commit {
                explain_since: Some(since),
                ..
            }) => COMMIT_REVEAL.checked_sub(since.elapsed()),
            _ => None,
        };
        // Terminal output held back by the frame cap.
        #[cfg(feature = "terminal")]
        let terminal = self.terminal_frame.next_wake(now);
        #[cfg(not(feature = "terminal"))]
        let terminal = None;
        // A task's output the live-scan throttle held back.
        #[cfg(feature = "terminal")]
        let task_scan = self.task_next_wake(now);
        #[cfg(not(feature = "terminal"))]
        let task_scan = None;
        [
            notif,
            terminal,
            task_scan,
            spinner,
            auto_save,
            caret,
            loading,
            nested_repositories,
            inlay,
            code_lens,
            links_colors,
            code_actions,
            inline_completion,
            occurrences,
            workspace_symbols,
            ai_commit,
            operation,
            reveal,
            self.drag_autoscroll_wake(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Every currently in-flight delayed-loading anchor the UI serves: the
    /// Source-Control sidebar's log/snapshot loads, the Search sidebar's running
    /// search, and each open tab's pending loads. This is the one enumeration
    /// behind the event loop's
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
        // A workspace search that has streamed nothing yet sends no event to wake the
        // loop, so its "searching…" status has to schedule its own reveal.
        if self.sidebar_visible
            && self.sidebar_panel == SidebarPanel::Search
            && self.search.searching.is_some()
        {
            pendings.extend(self.search.started);
        }
        pendings.extend(self.active_outline_loading());
        pendings.extend(self.quick_open_loading());
        pendings.extend(self.visible_hierarchy_loading());
        for tab in self.all_tabs() {
            if let Some(conflict) = tab
                .merge_conflict
                .as_ref()
                .filter(|conflict| conflict.current.is_none() && conflict.error.is_none())
            {
                pendings.push(conflict.loading_since);
            }
            // A code tab waiting for the session to load its text sends
            // nothing until the first snapshot, so its reveal is scheduled.
            if matches!(tab.kind, TabKind::Code { .. }) && tab.fill_caret.is_some() {
                pendings.extend(tab.fill_since);
            }
            pendings.extend(tab.kind.pendings());
        }
        // The GitHub surface is not a tab, so its waits have to be collected in their
        // own right — a page whose wait is invisible here never schedules its reveal
        // repaint, and its placeholder would appear only on the next keystroke.
        for page in self.github.pages() {
            pendings.extend(page.loading_since());
        }
        // A preview's images decode off-thread; their placeholders reveal on schedule too.
        pendings.extend(self.preview_images.pendings());
        pendings
    }
}
