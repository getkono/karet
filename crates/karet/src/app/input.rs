use super::*;

impl App {
    /// Selected text owned by the active Search or commit-message field.
    pub(super) fn modal_selection_text(&self) -> Option<String> {
        match self.input_context().modal? {
            Modal::CommitInput => self
                .commit_input
                .edit
                .selected_text(&self.commit_input.text)
                .map(str::to_string),
            Modal::SearchInput => {
                let (text, edit) = self.search.active_field_ref();
                edit.selected_text(text).map(str::to_string)
            },
            Modal::Find => {
                let find = self.active_find()?;
                match find.field {
                    SearchField::Find => find
                        .query_edit
                        .selected_text(&find.query)
                        .map(str::to_string),
                    SearchField::Replace => find
                        .replace_edit
                        .selected_text(&find.replace)
                        .map(str::to_string),
                }
            },
            Modal::ExplorerEdit => self.explorer.edit_selected_text().map(str::to_string),
            _ => None,
        }
    }

    /// Remove and return the active lightweight field's selected text.
    pub(super) fn cut_modal_selection(&mut self) -> Option<String> {
        match self.input_context().modal? {
            Modal::CommitInput => self.commit_input.edit.cut(&mut self.commit_input.text),
            Modal::SearchInput => {
                let (text, edit) = self.search.active_field();
                edit.cut(text)
            },
            Modal::Find => {
                let find = self.active_find_mut()?;
                match find.field {
                    SearchField::Find => find.query_edit.cut(&mut find.query),
                    SearchField::Replace => find.replace_edit.cut(&mut find.replace),
                }
            },
            Modal::ExplorerEdit => self.explorer.edit_cut(),
            _ => None,
        }
    }

    /// Select the entire active lightweight field, returning whether one owned focus.
    pub(super) fn select_all_modal_text(&mut self) -> bool {
        match self.input_context().modal {
            Some(Modal::CommitInput) => self.commit_input.edit.select_all(&self.commit_input.text),
            Some(Modal::SearchInput) => {
                let (text, edit) = self.search.active_field();
                let owned = text.clone();
                edit.select_all(&owned)
            },
            Some(Modal::Find) => {
                let Some(find) = self.active_find_mut() else {
                    return false;
                };
                match find.field {
                    SearchField::Find => find.query_edit.select_all(&find.query),
                    SearchField::Replace => find.replace_edit.select_all(&find.replace),
                }
            },
            Some(Modal::ExplorerEdit) => self.explorer.edit_select_all(),
            _ => return false,
        }
        true
    }

    /// Handle a key press: resolve it against the layered keymap for the current
    /// [input context](Self::input_context) and dispatch, or fall through to the
    /// active modal's text input when nothing is bound.
    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        // Typing means the pointer is no longer the subject; a Ctrl release that
        // produces no mouse event would otherwise leave the underline behind.
        self.definition_hover = None;
        let dismiss_outline_after = self.outline.overlay
            && self.focus == Focus::Editor
            && self.input_context().modal.is_none();
        if self.operation_blocker.is_some() {
            if key.code == KeyCode::Esc && key.modifiers.is_empty() {
                self.operation_blocker = None;
                self.status =
                    Some("quit cancelled; source control operation continues".to_string());
            }
            return;
        }
        if self.diagnostic_view.is_some() {
            self.diagnostic_view_key(key);
            return;
        }
        // Tab-driven key hooks only apply while the editor shell owns the content
        // area; in another view the active tab is off screen, and a key aimed at
        // the showing view must not reach it.
        if self.view == View::GitHub && self.input_context().modal.is_none() && self.github_key(key)
        {
            return;
        }
        // Esc dismisses a showing notification first (VS Code-style), but only when no
        // modal already owns Esc — so overlay/find/commit cancels are untouched, and
        // base Esc behaves normally whenever no toast is visible.
        if key.code == KeyCode::Esc
            && key.modifiers.is_empty()
            && !self.notifications.is_empty()
            && self.input_context().modal.is_none()
        {
            self.notifications.dismiss_latest();
            return;
        }
        let ctx = self.input_context();
        match ctx.modal {
            Some(modal) => match keymap::resolve(ctx, &[KeyChord::from_event(key)]) {
                Resolved::Command(command) => self.dispatch(command),
                Resolved::Pending | Resolved::None => self.modal_text(modal, key),
            },
            None => {
                // An open hover popup is dismissed by Esc before anything else
                // sees the key; every other key falls through (and most will
                // move the caret, which dismisses it anyway).
                if key.code == KeyCode::Esc && key.modifiers.is_empty() && self.dismiss_hover() {
                    return;
                }
                // The completion popup is a light key layer over the editor:
                // it consumes only its navigation/accept/dismiss keys and lets
                // everything else (typing, movement) fall through.
                if self.completion_key(key) {
                    return;
                }
                self.resolve_key(key);
            },
        }
        // Any key may have moved the caret or switched tabs; a popup or pending
        // request whose anchor no longer holds is dismissed.
        self.reconcile_completion();
        self.reconcile_hover();
        self.request_live_blame();
        if dismiss_outline_after {
            self.dismiss_outline_overlay();
        }
    }

    /// The current input context: the active modal (if any) over the focused pane.
    /// The precedence mirrors how the shell stacks these overlays. Also drives the
    /// context-aware status hints bar ([`crate::ui`]).
    pub(crate) fn input_context(&self) -> Context {
        let modal = if self.confirm.is_some() {
            // A question outranks the picker or menu it was raised from: answering
            // it is what everything under it is waiting on.
            Some(Modal::Confirm)
        } else if self.overlay.is_some() {
            Some(Modal::Overlay)
        } else if self.commit_input.focused {
            Some(Modal::CommitInput)
        } else if self.rev_input.is_some() {
            Some(Modal::RevInput)
        } else if self.context_menu.is_some() {
            Some(Modal::ContextMenu)
        } else if self.find_open {
            Some(Modal::Find)
        } else if self.explorer.is_editing() {
            Some(Modal::ExplorerEdit)
        } else if self.focus == Focus::Sidebar && self.sidebar_panel == SidebarPanel::Search {
            Some(if self.search.input {
                Modal::SearchInput
            } else {
                Modal::SearchList
            })
        } else {
            None
        };
        Context {
            modal,
            target: self.focus_target(),
        }
    }

    /// Resolve a focus-context key against the layered keymap, accumulating
    /// multi-key chord sequences. An unbound printable in the editor becomes text
    /// input; a broken sequence is dropped.
    pub(super) fn resolve_key(&mut self, key: KeyEvent) {
        self.pending.push(KeyChord::from_event(key));
        let ctx = Context::focus(self.focus_target());
        match keymap::resolve(ctx, &self.pending) {
            Resolved::Command(command) => {
                self.pending.clear();
                self.dispatch(command);
            },
            Resolved::Pending => {
                // A prefix of a longer binding: keep waiting. The status bar reads
                // `self.pending` directly to surface the typed chord and its
                // available completions (see `crate::ui::draw_status`).
            },
            Resolved::None => {
                let mid_sequence = self.pending.len() > 1;
                self.pending.clear();
                let plain = !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
                // The Seam view's query box takes raw characters, which its own layer
                // cannot express — every printable key would need a binding.
                if !mid_sequence && plain && self.view == View::Editor && self.seam_query_focused()
                {
                    match key.code {
                        KeyCode::Char(c) => self.seam_query_char(c),
                        KeyCode::Backspace => self.seam_query_backspace(),
                        _ => {},
                    }
                    return;
                }
                if !mid_sequence
                    && self.focus == Focus::Editor
                    // …and likewise for the unbound-printable fallback: a keystroke
                    // in another view must never land in a hidden document.
                    && self.view == View::Editor
                    && self.active_code_doc().is_some()
                    && plain
                    && let KeyCode::Char(c) = key.code
                {
                    self.dispatch(Command::InsertChar(c));
                }
            },
        }
    }

    /// Feed a key with no modal binding to the active modal's text input — the
    /// documented fall-through. The results list captures no text (unbound keys do
    /// nothing); an unbound key at the discard prompt cancels it.
    pub(super) fn modal_text(&mut self, modal: Modal, key: KeyEvent) {
        match modal {
            Modal::Overlay => self.overlay_input(key),
            Modal::Find => self.find_input(key),
            Modal::CommitInput => self.commit_edit(key),
            Modal::RevInput => self.rev_edit(key),
            Modal::ExplorerEdit => self.explorer_edit(key),
            Modal::SearchInput => self.search_edit(key),
            Modal::SearchList => {},
            Modal::ContextMenu => self.close_context_menu(),
            // An unbound key cancels a confirmation, matching every other confirm
            // prompt: the default answer to a question the user did not answer is no.
            Modal::Confirm => self.confirm_cancel(),
        }
    }

    /// Feed pasted text to the active modal's text field, mirroring `modal_text`
    /// for keys. Without this, paste always landed in the main editor buffer
    /// regardless of which text field was actually focused — corrupting the
    /// editor's selection with clipboard text meant for Find/Search/Commit/the
    /// explorer rename box/the quick-open query. A no-op for non-text modals.
    pub(super) fn modal_paste(&mut self, modal: Modal, text: &str) {
        match modal {
            Modal::Overlay => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.push_str(text);
                }
            },
            Modal::Find => {
                let Some(find) = self.active_find_mut() else {
                    return;
                };
                let editing_query = find.field == SearchField::Find;
                let (target, edit) = if editing_query {
                    (&mut find.query, &mut find.query_edit)
                } else {
                    (&mut find.replace, &mut find.replace_edit)
                };
                // Paste at the caret, replacing any selection, rather than
                // always appending.
                edit.insert(target, text);
                if editing_query {
                    self.run_find();
                }
            },
            Modal::CommitInput => {
                self.commit_paste(text);
            },
            Modal::RevInput => {
                if let Some(rev) = self.rev_input.as_mut() {
                    rev.push_str(text);
                }
            },
            Modal::ExplorerEdit => self.explorer.edit_paste(text),
            Modal::SearchInput => {
                let (target, edit) = self.search.active_field();
                edit.insert(target, text);
            },
            Modal::SearchList
            | Modal::ContextMenu
            // A confirmation captures no text: pasting into a question is
            // meaningless, and must not fall through to the editor underneath.
            | Modal::Confirm => {},
        }
    }

    /// Feed a key to the explorer inline name editor: printable characters extend the
    /// name, Backspace trims it, and Shift with a motion extends the selection
    /// (Enter/Esc are handled as bound commands).
    pub(super) fn explorer_edit(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Backspace => self.explorer.edit_backspace(),
            KeyCode::Delete => self.explorer.edit_delete(),
            KeyCode::Left => self.explorer.edit_left(shift),
            KeyCode::Right => self.explorer.edit_right(shift),
            KeyCode::Home => self.explorer.edit_home(shift),
            KeyCode::End => self.explorer.edit_end(shift),
            KeyCode::Char('a') | KeyCode::Char('A')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.explorer.edit_select_all();
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.explorer.edit_push(c);
            },
            _ => {},
        }
    }

    /// Accept the highlighted overlay row (open a file / run a command), then close.
    pub(super) fn overlay_accept(&mut self) {
        let event = match self.overlay.as_ref() {
            Some(overlay) => overlay.accept(),
            None => return,
        };
        self.overlay = None;
        self.handle_overlay_event(event);
    }

    /// Run what one accepted overlay row stands for. Split from
    /// [`overlay_accept`](Self::overlay_accept) so each follow-up — several of
    /// which now raise a confirmation rather than acting — is reachable on its own.
    pub(super) fn handle_overlay_event(&mut self, event: OverlayEvent) {
        match event {
            OverlayEvent::Close => {},
            OverlayEvent::AcceptFile(path) => self.open_path(&path),
            OverlayEvent::AcceptSeamRoot(root) => self.open_seam_view_at(root),
            OverlayEvent::AcceptLocation { path, position } => {
                self.jump_to_location(&path, position);
            },
            OverlayEvent::AcceptCommand(cmd) => self.dispatch(cmd),
            OverlayEvent::AcceptRebaseTodo { onto, steps } => {
                self.run_vcs_action(VcsAction::RebaseInteractive { onto, steps });
            },
            OverlayEvent::AcceptDiffTarget { rev, label } => {
                self.open_changes_with(&rev, &label);
            },
            OverlayEvent::AcceptBranch(target) => {
                self.guard_branch_switch(target);
            },
            OverlayEvent::AcceptCreateBranch(options) => {
                if options.name.trim().is_empty() || options.start_point.trim().is_empty() {
                    self.status =
                        Some("create branch: name and start point are required".to_string());
                } else {
                    self.run_vcs_action(VcsAction::CreateBranch(options));
                }
            },
            OverlayEvent::AcceptPullRequest { remote, number } => {
                self.run_vcs_action(VcsAction::CheckoutPullRequest { remote, number });
            },
            OverlayEvent::AcceptStash(options) => {
                self.run_vcs_action(VcsAction::StashPush(options));
            },
            OverlayEvent::AcceptAiCommit(options) => self.save_ai_commit_options(*options),
            OverlayEvent::AcceptStashAction(action) => match action {
                StashAction::Preview(reference) => {
                    self.run_vcs_action(VcsAction::StashPreview { reference });
                },
                StashAction::Apply(reference) => {
                    self.run_vcs_action(VcsAction::StashApply { reference });
                },
                StashAction::Pop(reference) => {
                    self.run_vcs_action(VcsAction::StashPop { reference });
                },
                StashAction::Drop(reference) => {
                    self.confirm_action(
                        format!("Drop stash {reference}?"),
                        "Permanently removes this stash entry and the changes it \
                         holds. There is no reflog for a dropped stash.",
                        "Keep the stash",
                        "Drop",
                        ConfirmAction::DropStash(reference),
                    );
                },
                StashAction::Branch(reference) => {
                    self.overlay = Some(Overlay::text(
                        "Branch from stash",
                        TextPurpose::StashBranch { reference },
                    ));
                },
            },
            OverlayEvent::AcceptText { purpose, text } => match purpose {
                TextPurpose::StashBranch { reference } => {
                    if text.trim().is_empty() {
                        self.status = Some("stash branch: enter a branch name".to_string());
                    } else {
                        self.run_vcs_action(VcsAction::StashBranch {
                            name: text,
                            reference,
                        });
                    }
                },
                TextPurpose::RenameBranch { old } => {
                    if text.trim().is_empty() {
                        self.status = Some("rename branch: enter a new name".to_string());
                    } else {
                        self.run_vcs_action(VcsAction::RenameBranch { old, new: text });
                    }
                },
                TextPurpose::TagCreate { rev } => {
                    if text.trim().is_empty() {
                        self.status = Some("tag: enter a name".to_string());
                    } else {
                        self.run_vcs_action(VcsAction::TagCreate {
                            name: text.trim().to_string(),
                            rev,
                            message: None,
                        });
                    }
                },
                TextPurpose::DebugEvaluate => self.debug_evaluate(text),
                TextPurpose::FilterLanguageServers => {
                    self.set_language_server_filter(text);
                },
            },
            OverlayEvent::AcceptDeleteLocalBranch(name) => {
                self.run_vcs_action(VcsAction::DeleteBranch { name });
            },
            OverlayEvent::AcceptDeleteRemoteBranch { remote, branch } => {
                self.confirm_action(
                    format!("Delete {remote}/{branch}?"),
                    format!(
                        "Deletes the branch from {remote} for everyone. Anyone \
                         without a local copy loses access to its commits."
                    ),
                    "Keep the branch",
                    format!("Delete {remote}/{branch}"),
                    ConfirmAction::DeleteRemoteBranch { remote, branch },
                );
            },
        }
    }

    /// Edit the overlay query with an unbound key (backspace / printable).
    pub(super) fn overlay_input(&mut self, key: KeyEvent) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Backspace => overlay.pop_char(),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                overlay.push_char(c);
            },
            _ => {},
        }
    }
}
