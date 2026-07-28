use super::*;

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

    /// Send a document command for the active code tab, if any.
    pub(super) fn send_doc_command(&mut self, make: impl FnOnce(DocumentId) -> SessionCommand) {
        let Some(doc) = self.active_code_doc() else {
            return;
        };
        let result = self.backend.as_ref().map(|backend| {
            let id = backend.next_id();
            backend.send(id, make(doc))
        });
        if let Some(Err(e)) = result {
            self.notify_backend_error(e);
        }
    }

    /// Handle a quit request through the unified close guard.
    pub(super) fn request_quit(&mut self) {
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
        let removed: HashSet<ViewId> = self.removed_tab_views(request).into_iter().collect();
        let surviving: HashSet<DocumentId> = self
            .all_tabs()
            .filter(|tab| !removed.contains(&tab.view))
            .filter_map(Self::tab_doc)
            .collect();
        let mut at_risk: Vec<DocumentId> = Vec::new();
        for tab in self.all_tabs().filter(|tab| removed.contains(&tab.view)) {
            let Some(doc) = Self::tab_doc(tab) else {
                continue;
            };
            if surviving.contains(&doc) || at_risk.contains(&doc) {
                continue;
            }
            // The document is fully dropped by this request; prompt only if it is
            // dirty (checked across every view, so per-tab flag skew can't hide it).
            if self
                .all_tabs()
                .any(|t| Self::tab_doc(t) == Some(doc) && t.dirty)
            {
                at_risk.push(doc);
            }
        }
        at_risk
    }

    /// Route an irreversible close through the unified unsaved-changes guard. When it
    /// would drop the last view of one or more dirty documents it arms the
    /// confirmation prompt (default: abort); otherwise it runs immediately.
    ///
    /// Quit additionally honors `files.confirmOnExit`; tab/pane closes are always
    /// guarded — silently discarding unsaved changes is the data-loss bug this fixes.
    pub(super) fn guarded_close(&mut self, request: CloseRequest) {
        if matches!(request, CloseRequest::Quit)
            && let Some(operation) = self.scm.operation.as_ref()
        {
            let label = format!("{operation:?}");
            self.operation_blocker = Some(OperationBlocker {
                label: label.clone(),
                deadline: Instant::now() + OPERATION_SHUTDOWN_TIMEOUT,
            });
            self.status = Some(format!(
                "quit waiting for source control operation {label} (maximum 60s)"
            ));
            return;
        }
        let at_risk = self.docs_at_risk(request);
        let honor_setting =
            !matches!(request, CloseRequest::Quit) || self.settings.files.confirm_on_exit;
        if at_risk.is_empty() || !honor_setting {
            self.execute_close(request);
        } else {
            self.pending_close = Some(request);
            self.status = Some(close_prompt_message(request, at_risk.len()));
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
        let mut requests: Vec<RequestId> = self
            .pending_commit_detail
            .iter()
            .filter_map(|(request, destination)| {
                let view = match destination {
                    CommitDest::Tab { view } | CommitDest::Browser { view, .. } => view,
                };
                views.contains(view).then_some(*request)
            })
            .collect();
        for request in &requests {
            self.pending_commit_detail.remove(request);
        }
        let latex_requests: Vec<RequestId> = self
            .latex_previews
            .iter()
            .filter_map(|(request, view)| views.contains(view).then_some(*request))
            .collect();
        for request in latex_requests {
            self.latex_previews.remove(&request);
            requests.push(request);
        }
        let preparing: Vec<RequestId> = self
            .pending_commit_preparation
            .iter()
            .filter_map(|(request, pending)| {
                let view = match &pending.destination {
                    CommitDest::Tab { view } | CommitDest::Browser { view, .. } => view,
                };
                views.contains(view).then_some(*request)
            })
            .collect();
        for request in preparing {
            if let Some(pending) = self.pending_commit_preparation.remove(&request) {
                pending
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            requests.push(request);
        }
        let verifications: Vec<RequestId> = self
            .pending_commit_verification
            .iter()
            .filter_map(|(request, (view, _))| views.contains(view).then_some(*request))
            .collect();
        for request in verifications {
            self.pending_commit_verification.remove(&request);
            requests.push(request);
        }
        if let Some((request, view)) = self.graph_log_req
            && views.contains(&view)
        {
            self.graph_log_req = None;
            requests.push(request);
        }
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
        let saved = self.save_docs(&at_risk);
        if saved == 0 {
            self.execute_close(request);
        } else {
            self.saving_close = Some(request);
            let verb = if matches!(request, CloseRequest::Quit) {
                "quitting"
            } else {
                "closing"
            };
            self.status = Some(format!("saving {saved} file(s) before {verb}…"));
        }
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
        self.status = Some(if quitting {
            "quit cancelled".to_string()
        } else {
            "close cancelled".to_string()
        });
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
    /// in `pending_saves` and marking its tabs as saving. Returns the number issued.
    pub(super) fn save_docs(&mut self, docs: &[DocumentId]) -> usize {
        let mut issued = 0;
        for &doc in docs {
            if self.send_save(doc) {
                issued += 1;
            }
        }
        issued
    }

    /// Send one save through the same backend path used by manual, close-guard, and
    /// automatic saves. The session owns the last-read fingerprint check, so every
    /// caller gets identical external-change protection.
    fn send_save(&mut self, doc: DocumentId) -> bool {
        let Some(backend) = self.backend.clone() else {
            return false;
        };
        if self
            .pending_saves
            .values()
            .any(|pending| pending.doc == doc)
        {
            return false;
        }
        let version = self.document_version(doc);
        let id = backend.next_id();
        match backend.send(id, SessionCommand::Save { doc }) {
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
            self.status = Some("save: open a text file".to_string());
            return;
        };
        if self
            .pending_saves
            .values()
            .any(|pending| pending.doc == doc)
        {
            self.status = Some("save already in progress".to_string());
            return;
        }
        self.send_save(doc);
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
        let focused = (self.focus == Focus::Editor)
            .then(|| self.active_code_doc())
            .flatten();
        if mode == AutoSave::OnFocusChange && focused != Some(doc) {
            self.save_docs(&[doc]);
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
        self.save_docs(&due);
    }

    /// Save the previously-focused editor document when a user action moves focus
    /// elsewhere or selects another document.
    pub(super) fn auto_save_context_changed(&mut self, previous: Option<DocumentId>) {
        if self.settings.files.auto_save != AutoSave::OnFocusChange {
            return;
        }
        let current = (self.focus == Focus::Editor)
            .then(|| self.active_code_doc())
            .flatten();
        if previous != current
            && let Some(doc) = previous
            && self.auto_save_pending.contains_key(&doc)
        {
            self.save_docs(&[doc]);
        }
    }

    /// Save the active editor document when the terminal window itself loses focus.
    pub(super) fn auto_save_focus_lost(&mut self) {
        if self.settings.files.auto_save == AutoSave::OnFocusChange
            && self.focus == Focus::Editor
            && let Some(doc) = self.active_code_doc()
            && self.auto_save_pending.contains_key(&doc)
        {
            self.save_docs(&[doc]);
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

    /// Cut the current selection (copy then delete); a no-op without a selection.
    pub(super) fn cut(&mut self) {
        if self.focus_target() == FocusTarget::Explorer {
            self.explorer_cut_files();
            return;
        }
        let has_selection = matches!(
            self.tabs.get(self.active),
            Some(Tab { kind: TabKind::Code { .. }, editor, .. })
                if editor.selection_range().is_some_and(|r| !r.is_empty())
        );
        if !has_selection {
            return;
        }
        self.copy_selection();
        self.submit_edit_with_cause(EditCause::Cut, editing::backspace);
    }

    /// Paste the system clipboard at the caret (or the active modal's text field).
    pub(super) fn paste_from_clipboard(&mut self) {
        if self.focus_target() == FocusTarget::Explorer {
            self.explorer_paste_files();
            return;
        }
        match self.clipboard.get() {
            Ok(text) => self.handle_paste(text),
            Err(_) => self.status = Some("paste: clipboard unavailable".to_string()),
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
        self.submit_edit_with_cause(EditCause::Paste, move |caret, sel, _b, base| {
            Some(editing::insert(caret, sel, base, &normalized))
        });
    }
}
