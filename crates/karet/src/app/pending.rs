//! What the event loop waits on: every delayed-loading anchor the app holds
//! ([`App::pendings`]), and the earliest moment something on screen must change
//! without input arriving.

use super::App;
use super::SidebarPanel;
use crate::pending::Pending;
use crate::tab::TabKind;

impl App {
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
