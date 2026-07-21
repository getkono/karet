use super::*;

impl App {
    /// Handle a key press: resolve it against the layered keymap for the current
    /// [input context](Self::input_context) and dispatch, or fall through to the
    /// active modal's text input when nothing is bound.
    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
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
        self.request_live_blame();
    }

    /// The current input context: the active modal (if any) over the focused pane.
    /// The precedence mirrors how the shell stacks these overlays. Also drives the
    /// context-aware status hints bar ([`crate::ui`]).
    pub(crate) fn input_context(&self) -> Context {
        let modal = if self.pending_swaps.is_some() {
            // A startup recovery decision blocks everything else until made.
            Some(Modal::SwapRecover)
        } else if self.pending_close.is_some() {
            Some(Modal::CloseConfirm)
        } else if self.overlay.is_some() {
            Some(Modal::Overlay)
        } else if self.commit_input.is_some() {
            Some(Modal::CommitInput)
        } else if self.rev_input.is_some() {
            Some(Modal::RevInput)
        } else if self.pending_discard.is_some() {
            Some(Modal::DiscardConfirm)
        } else if self.pending_explorer_delete.is_some() {
            Some(Modal::ExplorerDeleteConfirm)
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
                if !mid_sequence
                    && self.focus == Focus::Editor
                    && self.active_code_doc().is_some()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
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
            Modal::DiscardConfirm => self.resolve_discard(false),
            Modal::ExplorerDeleteConfirm => self.resolve_explorer_delete(false),
            Modal::ContextMenu => self.close_context_menu(),
            // An unbound key cancels the close prompt (stay in the editor); the
            // default for every irreversible close is to abort.
            Modal::CloseConfirm => self.cancel_close(),
            // …and dismisses the recovery prompt, keeping the swaps for a later launch.
            Modal::SwapRecover => {
                self.pending_swaps = None;
                self.status = Some("recovery dismissed (backups kept)".to_string());
            },
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
                let target = if editing_query {
                    &mut find.query
                } else {
                    &mut find.replace
                };
                target.push_str(text);
                if editing_query {
                    self.run_find();
                }
            },
            Modal::CommitInput => {
                if let Some(message) = self.commit_input.as_mut() {
                    message.push_str(text);
                }
            },
            Modal::RevInput => {
                if let Some(rev) = self.rev_input.as_mut() {
                    rev.push_str(text);
                }
            },
            Modal::ExplorerEdit => self.explorer.edit_paste(text),
            Modal::SearchInput => {
                let target = match self.search.field {
                    SearchField::Find => &mut self.search.query,
                    SearchField::Replace => &mut self.search.replace,
                };
                target.push_str(text);
            },
            Modal::SearchList
            | Modal::DiscardConfirm
            | Modal::ExplorerDeleteConfirm
            | Modal::ContextMenu
            | Modal::CloseConfirm
            | Modal::SwapRecover => {},
        }
    }

    /// Feed a key to the explorer inline name editor: printable characters extend the
    /// name, Backspace trims it (Enter/Esc are handled as bound commands).
    pub(super) fn explorer_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => self.explorer.edit_backspace(),
            KeyCode::Delete => self.explorer.edit_delete(),
            KeyCode::Left => self.explorer.edit_left(),
            KeyCode::Right => self.explorer.edit_right(),
            KeyCode::Home => self.explorer.edit_home(),
            KeyCode::End => self.explorer.edit_end(),
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
        match event {
            OverlayEvent::Close => {},
            OverlayEvent::AcceptFile(path) => self.open_path(&path),
            OverlayEvent::AcceptCommand(cmd) => self.dispatch(cmd),
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
                    self.overlay = Some(Overlay::text(
                        "Type drop to permanently remove the stash",
                        TextPurpose::ConfirmDropStash { reference },
                    ));
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
                TextPurpose::SaveAndSwitch { target } => {
                    if text == "save" {
                        self.save_then_switch(target);
                    } else {
                        self.status = Some("branch switch cancelled".to_string());
                    }
                },
                TextPurpose::StashAndSwitch { target } => {
                    if text == "stash" {
                        self.run_vcs_action(VcsAction::StashPush(
                            karet_vcs::StashOptions::default(),
                        ));
                        self.run_vcs_action(VcsAction::SwitchBranch(target));
                    } else {
                        self.status = Some("branch switch cancelled".to_string());
                    }
                },
                TextPurpose::ConfirmDropStash { reference } => {
                    if text == "drop" {
                        self.run_vcs_action(VcsAction::StashDrop { reference });
                    } else {
                        self.status = Some("stash drop cancelled".to_string());
                    }
                },
                TextPurpose::ConfirmPublishedUndo => {
                    if text == "undo" {
                        self.run_vcs_action(VcsAction::UndoCommit {
                            allow_upstream: true,
                        });
                    } else {
                        self.status = Some("undo commit cancelled".to_string());
                    }
                },
                TextPurpose::RenameBranch { old } => {
                    if text.trim().is_empty() {
                        self.status = Some("rename branch: enter a new name".to_string());
                    } else {
                        self.run_vcs_action(VcsAction::RenameBranch { old, new: text });
                    }
                },
                TextPurpose::ConfirmDeleteRemoteBranch { remote, branch } => {
                    if text == branch {
                        self.run_vcs_action(VcsAction::DeleteRemoteBranch { remote, branch });
                    } else {
                        self.status = Some("remote branch deletion cancelled".to_string());
                    }
                },
            },
            OverlayEvent::AcceptDeleteLocalBranch(name) => {
                self.run_vcs_action(VcsAction::DeleteBranch { name });
            },
            OverlayEvent::AcceptDeleteRemoteBranch { remote, branch } => {
                self.overlay = Some(Overlay::text(
                    format!("Type {branch} to delete {remote}/{branch}"),
                    TextPurpose::ConfirmDeleteRemoteBranch { remote, branch },
                ));
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
