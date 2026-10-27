use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

use karet_core::NotificationKind;
use karet_session::Command as SessionCommand;
use karet_session::DocumentCommand;
use karet_session::DocumentId;
use karet_session::RequestId;
use karet_session::SaveCause;
use karet_session::ViewId;
use karet_session::config::schema::AutoSave;

use crate::app::App;
use crate::app::OPERATION_SHUTDOWN_TIMEOUT;
use crate::app::confirm::ConfirmChoice;
use crate::app::confirm::ConfirmDialog;
use crate::app::notifications::Report;
use crate::app::state::CloseRequest;
use crate::app::state::OperationBlocker;
use crate::app::state::PendingAutoSave;
use crate::app::state::PendingSave;
use crate::app::util::close_prompt_choices;
use crate::command::Command;
use crate::keymap::Focus;
use crate::keymap::FocusTarget;
use crate::keymap::Modal;
use crate::tab::FindState;
use crate::tab::Tab;
use crate::tab::TabKind;

/// The most tab titles the close confirmation spells out before summarizing.
const NAMED_TABS: usize = 6;

impl App {
    /// The active tab's session document, if it is a registered code tab.
    pub(super) fn active_code_doc(&self) -> Option<DocumentId> {
        match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { doc: Some(doc), .. },
                ..
            }) => Some(*doc),
            _ => None,
        }
    }

    /// The active tab's find-in-file state, if any (find-in-file lives per tab so
    /// it survives closing the bar, but not closing the tab).
    pub(super) fn active_find(&self) -> Option<&FindState> {
        self.tabs.get(self.active)?.find.as_ref()
    }

    /// A mutable handle to the active tab's find-in-file state.
    pub(super) fn active_find_mut(&mut self) -> Option<&mut FindState> {
        self.tabs.get_mut(self.active)?.find.as_mut()
    }

    /// Send an undo (`undo`) or redo for the active code tab's document, if
    /// any, remembering this view as the one whose selection set the answer
    /// restores.
    pub(super) fn send_history(&mut self, undo: bool) {
        let Some(doc) = self.active_code_doc() else {
            return;
        };
        if let Some(tab) = self.tabs.get(self.active) {
            self.docs.history_view.insert(doc, tab.view);
        }
        let _ = self.send(if undo {
            DocumentCommand::Undo { doc }
        } else {
            DocumentCommand::Redo { doc }
        });
    }

    /// Handle a quit request through the unified close guard.
    pub(super) fn request_quit(&mut self) {
        // A plain quit is not an Open Recent reopen. One armed earlier may still
        // be set when its quit was abandoned without the prompt being cancelled
        // (a save before quitting failed, or Esc left a source control wait).
        self.cancel_relaunch();
        self.guarded_close(CloseRequest::Quit);
    }

    /// The stable view ids of the tabs `request` would drop. Tab/pane closes act on
    /// the focused pane only (mirroring the raw close operations); Quit drops every
    /// tab across every pane.
    pub(super) fn removed_tab_views(&self, request: CloseRequest) -> Vec<ViewId> {
        match request {
            CloseRequest::Quit => self.all_tabs().map(|tab| tab.view).collect(),
            CloseRequest::Tab { view } => vec![view],
            CloseRequest::OtherTabs => self
                .tabs
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != self.active)
                .map(|(_, tab)| tab.view)
                .collect(),
            CloseRequest::TabsToRight => self
                .tabs
                .iter()
                .skip(self.active + 1)
                .map(|tab| tab.view)
                .collect(),
            CloseRequest::AllTabs => self.tabs.iter().map(|tab| tab.view).collect(),
        }
    }

    /// The documents `request` would irreversibly lose: the dirty documents whose
    /// **last** referencing view is being dropped. A dirty document still shown in a
    /// surviving tab or another pane is not at risk, so closing
    /// one of its several views must not prompt.
    pub(super) fn docs_at_risk(&self, request: CloseRequest) -> Vec<DocumentId> {
        self.fully_dropped_docs(request)
            .into_iter()
            // Prompt only for a dirty document, checked across every view so that
            // per-tab flag skew cannot hide it.
            .filter(|doc| {
                self.all_tabs()
                    .any(|t| Self::tab_doc(t) == Some(*doc) && t.dirty)
            })
            .collect()
    }

    /// The documents `request` drops entirely: those whose **last** referencing view
    /// is being removed. A document still shown in a surviving tab or another pane is
    /// not dropped by it, so closing one of its several views loses nothing.
    pub(super) fn fully_dropped_docs(&self, request: CloseRequest) -> Vec<DocumentId> {
        let removed: HashSet<ViewId> = self.removed_tab_views(request).into_iter().collect();
        let surviving: HashSet<DocumentId> = self
            .all_tabs()
            .filter(|tab| !removed.contains(&tab.view))
            .filter_map(Self::tab_doc)
            .collect();
        let mut dropped: Vec<DocumentId> = Vec::new();
        for tab in self.all_tabs().filter(|tab| removed.contains(&tab.view)) {
            let Some(doc) = Self::tab_doc(tab) else {
                continue;
            };
            if surviving.contains(&doc) || dropped.contains(&doc) {
                continue;
            }
            dropped.push(doc);
        }
        dropped
    }

    /// How many saves are still in flight for documents `request` would drop. A close
    /// that outran one would lose a write the user explicitly asked for, so it parks
    /// on the drain instead of racing it.
    ///
    /// The same count releases a parked request in [`App::on_backend_event`]: park and
    /// release must read the *same* set of saves, or a close waits on writes it does
    /// not own — or, worse, runs while one it does own is still in flight.
    pub(super) fn saves_at_risk(&self, request: CloseRequest) -> usize {
        let dropped = self.fully_dropped_docs(request);
        self.pending_saves
            .values()
            .filter(|save| dropped.contains(&save.doc))
            .count()
    }

    /// Route an irreversible close through the unified unsaved-changes guard. When it
    /// would drop the last view of one or more dirty documents it arms the
    /// confirmation prompt (default: abort); otherwise it runs immediately.
    ///
    /// Quit additionally honors `files.confirmOnExit`; tab/pane closes are always
    /// guarded — silently discarding unsaved changes is the data-loss bug this fixes.
    /// Name the documents a close would lose, by the tab titles the user is
    /// already looking at. "2 unsaved files" says how much is at stake but not
    /// which, and which is the thing worth checking before discarding.
    fn at_risk_names(&self, at_risk: &[DocumentId]) -> String {
        let mut names: Vec<String> = Vec::new();
        for tab in self.all_tabs() {
            let Some(doc) = Self::tab_doc(tab) else {
                continue;
            };
            if at_risk.contains(&doc) && !names.contains(&tab.title) {
                names.push(tab.title.clone());
            }
        }
        let shown = names.len().min(NAMED_TABS);
        let mut listed = names[..shown].join(", ");
        if let Some(rest) = names.len().checked_sub(NAMED_TABS).filter(|n| *n > 0) {
            listed.push_str(&format!(", and {rest} more"));
        }
        listed
    }

    pub(super) fn guarded_close(&mut self, request: CloseRequest) {
        if matches!(request, CloseRequest::Quit)
            && let Some(operation) = self.scm.operation.as_ref()
        {
            // No notification here: `operation_blocker` is a modal that already
            // renders this label and the wait it implies.
            self.operation_blocker = Some(OperationBlocker {
                label: format!("{operation:?}"),
                deadline: Instant::now() + OPERATION_SHUTDOWN_TIMEOUT,
            });
            return;
        }
        let at_risk = self.docs_at_risk(request);
        let honor_setting =
            !matches!(request, CloseRequest::Quit) || self.settings.files.confirm_on_exit;
        if at_risk.is_empty() || !honor_setting {
            // A close that never raises the prompt still cannot outrun a write it
            // already owes. A save deferred on a formatter answers seconds after
            // it was asked for, and closing first drops it with nothing to show
            // for the keystroke, so park on the same drain the prompt uses. Count
            // only the writes *this* request would drop: a tab close owes nothing
            // for a document it leaves open elsewhere. Ctrl+S on a buffer with no
            // unsaved edits still issues a real save -- it is how a reformat is
            // asked for -- so a clean document arrives here with a write pending
            // and no prompt above to have held it.
            let in_flight = self.saves_at_risk(request);
            if in_flight > 0 {
                // Unless the user is already waiting on a parked quit. Nothing
                // else here releases one -- only the backend answering does --
                // so a second Ctrl+Q is the only way out of a wedged session,
                // and refusing it would make the editor unexitable. Only a quit
                // arms that hatch and only a parked quit trips it: a parked tab
                // close is a different request, and letting it stand in would
                // abandon writes on the user's *first* Ctrl+Q.
                let forcing = matches!(request, CloseRequest::Quit)
                    && matches!(self.saving_close, Some(CloseRequest::Quit));
                if !forcing {
                    self.park_close_on_saves(request, in_flight);
                    return;
                }
                self.saving_close = None;
                self.notifications.dismiss_tagged(Self::SAVE_BATCH_TAG);
                // What makes an abandoned write recoverable is the swap file the
                // session wrote for it -- and `files.backup = false` means it wrote
                // none. Promising recovery there would tell the user their work is
                // safe at the exact moment it is destroyed, so the two cases get
                // two messages. Both stay on the Failure tier, which is already the
                // loudest one (red, persists until dismissed); the wording is what
                // has to carry the difference.
                let report = if self.settings.files.backup {
                    format!("quit: {in_flight} save(s) abandoned, recoverable from swap files")
                } else {
                    format!(
                        "quit: {in_flight} save(s) abandoned and lost — files.backup is off, \
                         so no swap file was written"
                    )
                };
                // The loop exits before another frame is drawn, so the toast alone
                // is never seen; the farewell is printed once the terminal is back.
                self.farewell.push(report.clone());
                self.notify(Report::Failure, NotificationKind::Io, report);
            }
            self.execute_close(request);
        } else {
            let names = self.at_risk_names(&at_risk);
            let (title, save, discard) = close_prompt_choices(request, at_risk.len());
            // Open first, arm second. Opening declines whatever dialog it replaces,
            // and a close prompt's own decline clears `pending_close` — so arming
            // before this call would have the outgoing dialog wipe the request the
            // incoming one depends on, leaving its answers inert.
            self.confirm(ConfirmDialog::new(
                title,
                format!("Unsaved changes in {names} have not been written to disk."),
                vec![
                    ConfirmChoice::custom("Cancel", Command::CloseConfirmCancel),
                    ConfirmChoice::custom(save, Command::CloseConfirmSave),
                    ConfirmChoice::custom(discard, Command::CloseConfirmDiscard),
                ],
            ));
            self.pending_close = Some(request);
        }
    }

    /// Run a confirmed (or unguarded) close, re-resolving a single-tab request by its
    /// view id so a save-then-close that shifted the tab list still closes the right
    /// tab (and harmlessly no-ops if it has since vanished).
    pub(super) fn execute_close(&mut self, request: CloseRequest) {
        let removed: HashSet<ViewId> = self.removed_tab_views(request).into_iter().collect();
        self.cancel_loading_for_views(&removed);
        match request {
            CloseRequest::Quit => self.should_quit = true,
            CloseRequest::Tab { view } => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.view == view) {
                    self.close_tab_at(index);
                }
            },
            CloseRequest::OtherTabs => self.close_other_tabs(),
            CloseRequest::TabsToRight => self.close_tabs_to_right(),
            CloseRequest::AllTabs => self.close_all_tabs(),
        }
    }

    /// Cancel safely-droppable backend reads owned exclusively by closing views.
    /// The views close immediately; cancelled ids remain tombstoned so already
    /// queued progressive responses cannot recreate a tab.
    pub(super) fn cancel_loading_for_views(&mut self, views: &HashSet<ViewId>) {
        let abandoned: Vec<RequestId> = self
            .pending_open
            .iter()
            .filter_map(|(request, pending)| views.contains(&pending.view).then_some(*request))
            .collect();
        for request in abandoned {
            self.pending_open.remove(&request);
            self.abandoned_open.insert(request);
        }
        let mut requests = Vec::new();
        drain_view_requests(
            &mut self.pending_commit_detail,
            views,
            &mut requests,
            |view| *view,
        );
        drain_view_requests(&mut self.pending_compare, views, &mut requests, |view| {
            *view
        });
        drain_view_requests(&mut self.latex_previews, views, &mut requests, |view| *view);
        drain_view_requests(
            &mut self.pending_prepared_diffs,
            views,
            &mut requests,
            |(view, _)| *view,
        );
        drain_view_requests(&mut self.pending_conversions, views, &mut requests, |v| *v);
        drain_view_requests(
            &mut self.pending_commit_verification,
            views,
            &mut requests,
            |(view, _)| *view,
        );
        drain_view_requests(
            &mut self.pending_merge_conflicts,
            views,
            &mut requests,
            |(view, _)| *view,
        );
        drain_view_requests(&mut self.graph_log_reqs, views, &mut requests, |view| *view);
        for request in requests {
            self.cancel_backend_request(request);
        }
    }

    /// Tombstone and cooperatively cancel one safely-droppable backend request.
    pub(super) fn cancel_backend_request(&mut self, request: RequestId) {
        self.cancelled_requests.insert(request);
        // IDs are monotonic. Bound stale-response tombstones while retaining a wide
        // window for progressive events that were already queued at cancellation.
        let floor = request.0.saturating_sub(1024);
        self.cancelled_requests.retain(|id| id.0 >= floor);
        self.send_command(SessionCommand::Cancel { request });
    }

    /// At the close prompt: save exactly the at-risk documents, then run the parked
    /// request once those saves drain (see [`App::on_backend_event`]). Runs
    /// immediately if nothing needed saving.
    pub(super) fn close_save(&mut self) {
        let Some(request) = self.pending_close.take() else {
            return;
        };
        let at_risk = self.docs_at_risk(request);
        let saved = self.save_docs(&at_risk, SaveCause::Manual);
        if saved == 0 {
            self.execute_close(request);
        } else {
            self.park_close_on_saves(request, saved);
        }
    }

    /// Hold `request` until every save in flight has answered, and say so. The drain
    /// in [`App::on_backend_event`] runs it; a failed save cancels it there instead.
    fn park_close_on_saves(&mut self, request: CloseRequest, count: usize) {
        self.saving_close = Some(request);
        let verb = if matches!(request, CloseRequest::Quit) {
            "quitting"
        } else {
            "closing"
        };
        self.notify_progress(
            NotificationKind::Io,
            Self::SAVE_BATCH_TAG.to_string(),
            format!("saving {count} file(s) before {verb}…"),
            None,
        );
    }

    /// At the close prompt: discard unsaved changes and run the parked request now.
    pub(super) fn close_discard(&mut self) {
        if let Some(request) = self.pending_close.take() {
            self.execute_close(request);
        }
    }

    /// At the close prompt: an unbound key aborts, leaving every tab untouched.
    pub(super) fn cancel_close(&mut self) {
        let quitting = matches!(self.pending_close, Some(CloseRequest::Quit));
        self.pending_close = None;
        // An Open Recent reopen rides on the quit it just cancelled.
        self.cancel_relaunch();
        self.notify(
            Report::Outcome,
            NotificationKind::System,
            if quitting {
                "quit cancelled"
            } else {
                "close cancelled"
            },
        );
    }

    /// Finish a timed graceful-shutdown wait. Once the global ceiling is reached,
    /// terminate rather than leaving the terminal trapped indefinitely.
    pub(super) fn expire_operation_blocker(&mut self, now: Instant) {
        if self
            .operation_blocker
            .as_ref()
            .is_some_and(|blocker| now >= blocker.deadline)
        {
            self.operation_blocker = None;
            self.should_quit = true;
        }
    }

    /// Issue a save for each of `docs` (skipping any already in flight), tracking it
    /// in `pending_saves` and marking its tabs as saving. Returns how many of `docs`
    /// a write is now owed for.
    ///
    /// That count is documents *covered*, not requests issued: a document whose save
    /// is already in flight is one this call must still be waited on for. Callers park
    /// destructive work on a non-zero count, and a save deferred on a formatter can be
    /// in flight for seconds — so counting only the new requests would run the close,
    /// or the branch switch, while the write it was guarding is still parked.
    pub(super) fn save_docs(&mut self, docs: &[DocumentId], cause: SaveCause) -> usize {
        let mut covered = 0;
        for &doc in docs {
            if self.send_save(doc, cause) || self.save_in_flight(doc) {
                covered += 1;
            }
        }
        covered
    }

    /// Whether a save for `doc` is already awaiting its answering event.
    pub(super) fn save_in_flight(&self, doc: DocumentId) -> bool {
        self.pending_saves
            .values()
            .any(|pending| pending.doc == doc)
    }

    /// Send one save through the same backend path used by manual, close-guard, and
    /// automatic saves. The session owns the last-read fingerprint check, so every
    /// caller gets identical external-change protection.
    fn send_save(&mut self, doc: DocumentId, cause: SaveCause) -> bool {
        let Some(backend) = self.backend.clone() else {
            return false;
        };
        if self.save_in_flight(doc) {
            return false;
        }
        let version = self.document_version(doc);
        // A save may rewrite the buffer (formatting, whitespace cleanup). Tell
        // the backend where the caret is first, so undoing that rewrite puts
        // the caret back here rather than at the top of the file. The focused
        // tab's caret wins when it shows this document. A failed send is left
        // to the save below, which reports the same backend error.
        let is_doc =
            |tab: &&Tab| matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc);
        if let Some(tab) = self
            .tabs
            .get(self.active)
            .filter(is_doc)
            .or_else(|| self.all_tabs().find(is_doc))
        {
            let cursors = tab.editor.cursors().clone();
            let _ = backend.send(
                backend.next_id(),
                DocumentCommand::SetCursor {
                    doc,
                    view: tab.view,
                    cursors,
                }
                .into(),
            );
        }
        let id = backend.next_id();
        match backend.send(id, DocumentCommand::Save { doc, cause }.into()) {
            Ok(()) => {
                self.pending_saves.insert(id, PendingSave { doc });
                if self
                    .auto_save_pending
                    .get(&doc)
                    .is_some_and(|pending| pending.version <= version)
                {
                    self.auto_save_pending.remove(&doc);
                }
                let now = Instant::now();
                for tab in self.all_tabs_mut() {
                    if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                        tab.saving_since = Some(now);
                    }
                }
                true
            },
            Err(e) => {
                self.notify_backend_error(e);
                false
            },
        }
    }

    fn document_version(&self, doc: DocumentId) -> u64 {
        self.all_tabs()
            .filter_map(|tab| match &tab.kind {
                TabKind::Code {
                    doc: Some(candidate),
                    next_version,
                    ..
                } if *candidate == doc => Some(*next_version),
                _ => None,
            })
            .max()
            .unwrap_or_default()
    }

    /// Save the active document, or report that there is no file to save. Tracks the
    /// in-flight save so a slow write shows a spinner in the tab.
    pub(super) fn save_active(&mut self) {
        let Some(doc) = self.active_code_doc() else {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "save: open a text file",
            );
            return;
        };
        if self.save_in_flight(doc) {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "save already in progress",
            );
            return;
        }
        self.send_save(doc, SaveCause::Manual);
    }

    /// Record a new dirty version for the configured automatic-save trigger. A
    /// repeated snapshot for the same version does not restart the inactivity timer.
    pub(super) fn schedule_auto_save(&mut self, doc: DocumentId, version: u64, now: Instant) {
        let mode = self.settings.files.auto_save;
        let deadline = match mode {
            AutoSave::Off => {
                self.auto_save_pending.remove(&doc);
                return;
            },
            AutoSave::AfterDelay => Some(
                now.checked_add(Duration::from_millis(self.settings.files.auto_save_delay))
                    .unwrap_or(now),
            ),
            AutoSave::OnFocusChange => None,
        };
        if self
            .auto_save_pending
            .get(&doc)
            .is_some_and(|pending| pending.version >= version)
        {
            return;
        }
        self.auto_save_pending
            .insert(doc, PendingAutoSave { version, deadline });
        let focused = (self.keyboard.focus() == Focus::Editor)
            .then(|| self.active_code_doc())
            .flatten();
        if mode == AutoSave::OnFocusChange && focused != Some(doc) {
            self.save_docs(&[doc], SaveCause::FocusChange);
        }
    }

    /// Fire every elapsed inactivity save. Called by the event loop after its timer
    /// wake, and exposed to unit tests with an explicit clock.
    pub(super) fn fire_auto_save(&mut self, now: Instant) {
        let due: Vec<DocumentId> = self
            .auto_save_pending
            .iter()
            .filter_map(|(doc, pending)| {
                (pending.deadline.is_some_and(|deadline| deadline <= now)
                    && !self.pending_saves.values().any(|save| save.doc == *doc))
                .then_some(*doc)
            })
            .collect();
        for doc in &due {
            self.auto_save_pending.remove(doc);
        }
        self.save_docs(&due, SaveCause::AutoDelay);
    }

    /// Save the previously-focused editor document when a user action moves focus
    /// elsewhere or selects another document.
    pub(super) fn auto_save_context_changed(&mut self, previous: Option<DocumentId>) {
        if self.settings.files.auto_save != AutoSave::OnFocusChange {
            return;
        }
        let current = (self.keyboard.focus() == Focus::Editor)
            .then(|| self.active_code_doc())
            .flatten();
        if previous != current
            && let Some(doc) = previous
            && self.auto_save_pending.contains_key(&doc)
        {
            self.save_docs(&[doc], SaveCause::FocusChange);
        }
    }

    /// Save the active editor document when the terminal window itself loses focus.
    pub(super) fn auto_save_focus_lost(&mut self) {
        if self.settings.files.auto_save == AutoSave::OnFocusChange
            && self.keyboard.focus() == Focus::Editor
            && let Some(doc) = self.active_code_doc()
            && self.auto_save_pending.contains_key(&doc)
        {
            self.save_docs(&[doc], SaveCause::FocusChange);
        }
    }

    /// Reconcile pending triggers after a live configuration change.
    pub(super) fn reconcile_auto_save_settings(&mut self, now: Instant) {
        if self.settings.files.auto_save == AutoSave::Off {
            self.auto_save_pending.clear();
            return;
        }
        let mut versions: HashMap<DocumentId, u64> = self
            .auto_save_pending
            .iter()
            .map(|(doc, pending)| (*doc, pending.version))
            .collect();
        for tab in self.all_tabs().filter(|tab| tab.dirty) {
            if let TabKind::Code {
                doc: Some(doc),
                next_version,
                ..
            } = &tab.kind
            {
                versions
                    .entry(*doc)
                    .and_modify(|version| *version = (*version).max(*next_version))
                    .or_insert(*next_version);
            }
        }
        self.auto_save_pending.clear();
        for (doc, version) in versions {
            self.schedule_auto_save(doc, version, now);
        }
    }

    /// Cut every selection (copy then delete), or every caret's whole line when
    /// nothing is selected.
    pub(super) fn cut(&mut self) {
        if matches!(
            self.input_context().modal,
            Some(
                Modal::SearchInput
                    | Modal::CommitInput
                    | Modal::Find
                    | Modal::ExplorerEdit
                    | Modal::Rename
            )
        ) {
            let Some(text) = self.cut_modal_selection() else {
                return;
            };
            self.copy_to_clipboard(text, "selection");
            return;
        }
        if self.focus_target() == FocusTarget::Explorer {
            self.explorer_cut_files();
            return;
        }
        self.cut_code_selection();
    }

    /// Paste the system clipboard at the caret (or the active modal's text field).
    pub(super) fn paste_from_clipboard(&mut self) {
        if self.focus_target() == FocusTarget::Explorer {
            self.explorer_paste_files();
            return;
        }
        match self.clipboard.get() {
            Ok(text) => self.handle_paste(text),
            Err(_) => self.notify(
                Report::Failure,
                NotificationKind::System,
                "paste: clipboard unavailable",
            ),
        }
    }

    /// Route pasted text (from the paste command or bracketed paste) to whatever
    /// actually owns text input right now: the active modal's field if one is
    /// open, else the editor buffer. Shared by both paste sources, so pasted text
    /// is never interpreted as keys and never lands in the wrong place.
    pub(super) fn handle_paste(&mut self, text: String) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        if normalized.is_empty() {
            return;
        }
        if let Some(modal) = self.input_context().modal {
            self.modal_paste(modal, &normalized);
            return;
        }
        #[cfg(feature = "terminal")]
        if self.terminal_owns_keyboard() {
            self.terminal_paste(&normalized);
            return;
        }
        self.paste_into_editor(&normalized);
    }
}

/// Remove from `pending` every request owned by one of `views` (read through
/// `view_of`), recording the removed ids in `cancelled`. The one shape behind
/// [`App::cancel_loading_for_views`]: each request-registry map stores its own
/// payload, but all of them are keyed by [`RequestId`] and owned by exactly one
/// view.
fn drain_view_requests<T>(
    pending: &mut HashMap<RequestId, T>,
    views: &HashSet<ViewId>,
    cancelled: &mut Vec<RequestId>,
    view_of: impl Fn(&T) -> ViewId,
) {
    let matching: Vec<RequestId> = pending
        .iter()
        .filter_map(|(request, value)| views.contains(&view_of(value)).then_some(*request))
        .collect();
    for request in matching {
        pending.remove(&request);
        cancelled.push(request);
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::harness::*;
    use crate::app::*;

    /// A line copy that comes back with CRLF endings (as a Windows or terminal
    /// clipboard hands it over) is still recognised as karet's own copy, so it
    /// pastes above the caret's line rather than at the caret.
    #[test]
    fn a_crlf_line_copy_still_pastes_as_a_line() {
        let mut rig = Rig::new("a|b\ncd");
        rig.step(Step::Key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        rig.step(key(KeyCode::Down));
        rig.app.handle_paste("ab\r\n".to_owned());
        assert_eq!(rig.marked(), "ab\nab\nc|d");
    }

    /// Every CRLF and lone CR becomes LF before the text lands; text that
    /// normalises to nothing changes nothing.
    #[test]
    fn pasted_line_endings_become_lf() {
        let rows = [
            ("a lone CR", "a\rb", "a\nb|"),
            ("a CRLF", "a\r\nb", "a\nb|"),
            ("mixed endings", "a\r\nb\rc\nd", "a\nb\nc\nd|"),
            ("CR CR stays two breaks", "a\r\rb", "a\n\nb|"),
            ("nothing", "", "|"),
        ];
        for (name, pasted, expect) in rows {
            let mut rig = Rig::new("|");
            rig.app.handle_paste(pasted.to_owned());
            assert_eq!(rig.marked(), expect, "{name}");
        }
    }
}
