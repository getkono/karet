//! The confirmation dialog: the app's single way to ask before it acts.
//!
//! Every consequential action routes through here rather than inventing its own
//! prompt. A dialog names what is about to happen in its title, spells the
//! consequence out in its body, and offers the answers as rows — so the user
//! reads the cost instead of recalling a magic word, and a screen reader or a
//! narrow terminal still gets the whole question.
//!
//! The safety property is structural, not a matter of care at each call site:
//! [`Dialog::new`](karet_widgets::Dialog::new) selects the first activatable
//! choice, and [`confirm`](App::confirm) is documented to take the safe answer
//! first. So `Enter` on an unread dialog runs that safe answer, and so does
//! every way of backing out — `Esc`, an unbound key (`modal_text` in
//! [`super::input`]), a click outside the box. Backing out is *taking the first
//! answer*, not a fourth outcome that skips it, which is what lets a dialog park
//! its own cleanup there (clearing a parked close, keeping crash backups) and
//! know it runs however the user declines. Reaching any other choice costs a
//! deliberate keystroke.
//!
//! The seam is the [context menu](karet_widgets::menu)'s: the widget owns the
//! model and the painting, while resolving what a row *says* and what accepting
//! it *does* stays here (see [`ui::confirm`](crate::ui::confirm)).

use super::*;

/// What accepting one confirmation row does.
///
/// Rows that map onto an existing named command carry it directly; actions that
/// need a payload the command vocabulary cannot express get their own variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmAction {
    /// Close the dialog and do nothing. Always offered, always first.
    Cancel,
    /// Dispatch a named application command.
    Command(Command),
    /// Throw away the working-tree changes to these paths.
    DiscardPaths(Vec<PathBuf>),
    /// Delete these explorer entries from disk (recursively, for directories).
    DeleteExplorerPaths(Vec<PathBuf>),
    /// Save every dirty editor, then switch to this branch.
    SaveAndSwitch(karet_vcs::BranchTarget),
    /// Stash the worktree, then retry the refused switch to this branch.
    StashAndSwitch(karet_vcs::BranchTarget),
    /// Permanently remove this stash entry.
    DropStash(String),
    /// Undo a commit that is already present upstream.
    UndoPublishedCommit,
    /// Hard-reset the worktree to this revision.
    ResetHard(String),
    /// Download and activate a missing managed language server.
    InstallLanguageServer(LanguageServerId),
    /// Record that the user does not want this provider offered again.
    DeclineLanguageServer(LanguageServerId),
    /// Deactivate a Karet-managed provider and retire its payload.
    UninstallLanguageServer(LanguageServerId),
    /// Apply the exact update plan the backend resolved.
    ApplyLanguageServerPlan {
        /// The plan the backend is holding.
        plan: LanguageServerPlanId,
        /// The providers from it to apply.
        servers: Vec<LanguageServerId>,
    },
    /// Open a file the link pointed to from outside the workspace.
    OpenOutsideWorkspaceLink(PathBuf),
    /// Create the project settings file, then add `word` to its dictionary.
    CreateProjectDictionary {
        /// The word that prompted the file's creation.
        word: String,
        /// The settings file to create.
        path: PathBuf,
    },
    /// Delete this branch from the remote.
    DeleteRemoteBranch {
        /// The remote holding the branch.
        remote: String,
        /// The branch to delete.
        branch: String,
    },
}

impl From<Command> for ConfirmAction {
    fn from(command: Command) -> Self {
        Self::Command(command)
    }
}

/// One answer row of the app's confirmation dialog.
pub(crate) type ConfirmChoice = karet_widgets::Choice<ConfirmAction>;
/// The app's confirmation dialog (the shared widget over [`ConfirmAction`]).
pub(crate) type ConfirmDialog = karet_widgets::Dialog<ConfirmAction>;

/// The label shown for a row that did not carry one of its own.
pub(crate) fn confirm_label(action: &ConfirmAction) -> String {
    match action {
        ConfirmAction::Cancel => "Cancel".to_string(),
        ConfirmAction::Command(command) => command.label().to_string(),
        ConfirmAction::DiscardPaths(_) => "Discard".to_string(),
        ConfirmAction::DeleteExplorerPaths(_) => "Delete".to_string(),
        ConfirmAction::SaveAndSwitch(_) => "Save all and switch".to_string(),
        ConfirmAction::StashAndSwitch(_) => "Stash and switch".to_string(),
        ConfirmAction::DropStash(_) => "Drop".to_string(),
        ConfirmAction::UndoPublishedCommit => "Undo".to_string(),
        ConfirmAction::ResetHard(_) => "Reset".to_string(),
        ConfirmAction::InstallLanguageServer(_) => "Install".to_string(),
        ConfirmAction::DeclineLanguageServer(_) => "Never ask again".to_string(),
        ConfirmAction::UninstallLanguageServer(_) => "Uninstall".to_string(),
        ConfirmAction::ApplyLanguageServerPlan { .. } => "Update".to_string(),
        ConfirmAction::OpenOutsideWorkspaceLink(_) => "Open".to_string(),
        ConfirmAction::CreateProjectDictionary { .. } => "Create".to_string(),
        ConfirmAction::DeleteRemoteBranch { .. } => "Delete".to_string(),
    }
}

/// The most file names a confirmation body spells out before summarizing. Past
/// this the list stops informing and starts pushing the answers off a short
/// terminal, and the count alone carries the weight.
const NAMED_PATHS: usize = 6;

/// Name the paths an action is about, relative to `root` where possible.
///
/// A confirmation that says "2 files" tells the user how much is at stake but
/// not whether it is the right two, which is exactly the check they opened the
/// dialog to make.
pub(crate) fn describe_paths(paths: &[PathBuf], root: &Path) -> String {
    let name = |path: &PathBuf| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let mut lines: Vec<String> = paths.iter().take(NAMED_PATHS).map(name).collect();
    if let Some(rest) = paths.len().checked_sub(NAMED_PATHS).filter(|n| *n > 0) {
        lines.push(format!("and {rest} more"));
    }
    lines.join(", ")
}

impl App {
    /// Open `dialog`, replacing any confirmation already up.
    ///
    /// The caller lists the **safe** answer first: the widget selects the first
    /// activatable row, so that choice is what `Enter` runs on a dialog the user
    /// has not read.
    pub(crate) fn confirm(&mut self, dialog: ConfirmDialog) {
        // Displacing a question is a way of not answering it, so the one being
        // replaced is declined rather than dropped. A backend event can arrive
        // while a dialog is up — a language server reporting itself missing, say,
        // over an unanswered close prompt — and without this the close would stay
        // parked in `pending_close` with nothing left on screen to release it.
        self.confirm_cancel();
        // A confirmation outranks a context menu, and the menu's rows would
        // otherwise keep painting under it.
        self.context_menu = None;
        self.confirm = Some(dialog);
    }

    /// Build and open a two-answer confirmation: cancel first, then `action`.
    pub(crate) fn confirm_action(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        cancel: impl Into<String>,
        proceed: impl Into<String>,
        action: ConfirmAction,
    ) {
        self.confirm(ConfirmDialog::new(
            title,
            body.into(),
            vec![
                ConfirmChoice::custom(cancel, ConfirmAction::Cancel),
                ConfirmChoice::custom(proceed, action),
            ],
        ));
    }

    /// Move the confirmation selection by `delta` rows.
    pub(super) fn confirm_step(&mut self, delta: i32) {
        if let Some(dialog) = self.confirm.as_mut() {
            dialog.select_by(delta);
        }
    }

    /// Decline the confirmation: close it and run its first answer.
    ///
    /// Declining is not a fourth outcome — it *is* the safe answer, which by
    /// construction sits first. A dialog that must undo something on the way out
    /// (drop a parked close, keep crash backups) puts that in row zero and gets
    /// it run however the user backs out.
    pub(super) fn confirm_cancel(&mut self) {
        let Some(dialog) = self.confirm.take() else {
            return;
        };
        let Some(action) = dialog
            .choices
            .entries
            .first()
            .map(|choice| choice.action.clone())
        else {
            return;
        };
        self.run_confirmed(action);
    }

    /// Close the confirmation and run whatever the selected row stands for.
    pub(super) fn confirm_accept(&mut self) {
        let Some(dialog) = self.confirm.take() else {
            return;
        };
        let Some(action) = dialog.selected_choice().map(|choice| choice.action.clone()) else {
            return;
        };
        self.run_confirmed(action);
    }

    /// Run a confirmed action. Split from [`confirm_accept`](Self::confirm_accept)
    /// so the mouse path can share it.
    pub(super) fn run_confirmed(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::Cancel => {},
            ConfirmAction::Command(command) => self.dispatch(command),
            ConfirmAction::DiscardPaths(paths) => self.discard_paths(paths),
            ConfirmAction::DeleteExplorerPaths(paths) => self.delete_explorer_paths(paths),
            ConfirmAction::SaveAndSwitch(target) => self.save_then_switch(target),
            ConfirmAction::StashAndSwitch(target) => {
                self.run_vcs_action(VcsAction::StashPush(karet_vcs::StashOptions::default()));
                self.run_vcs_action(VcsAction::SwitchBranch(target));
            },
            ConfirmAction::DropStash(reference) => {
                self.run_vcs_action(VcsAction::StashDrop { reference });
            },
            ConfirmAction::UndoPublishedCommit => {
                self.run_vcs_action(VcsAction::UndoCommit {
                    allow_upstream: true,
                });
            },
            ConfirmAction::ResetHard(rev) => {
                self.run_vcs_action(VcsAction::Reset {
                    mode: karet_vcs::ResetMode::Hard,
                    rev,
                });
            },
            ConfirmAction::InstallLanguageServer(server) => {
                self.begin_language_server_install(server);
            },
            ConfirmAction::DeclineLanguageServer(server) => {
                self.decline_language_server(server);
            },
            ConfirmAction::UninstallLanguageServer(server) => {
                self.begin_language_server_uninstall(server);
            },
            ConfirmAction::ApplyLanguageServerPlan { plan, servers } => {
                self.apply_language_server_plan(plan, servers, false);
            },
            ConfirmAction::OpenOutsideWorkspaceLink(path) => {
                self.open_markdown_file_link(&path);
            },
            ConfirmAction::CreateProjectDictionary { word, path } => {
                self.create_project_dictionary(&word, &path);
            },
            ConfirmAction::DeleteRemoteBranch { remote, branch } => {
                self.run_vcs_action(VcsAction::DeleteRemoteBranch { remote, branch });
            },
        }
    }

    /// Route a mouse event at the open confirmation, reporting whether it was
    /// consumed. An open dialog swallows everything: a click on a row runs it, a
    /// click anywhere else cancels, and motion becomes hover feedback.
    pub(in crate::app) fn handle_confirm_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(dialog) = self.confirm.as_ref() else {
            return false;
        };
        let point = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if rect_contains(dialog.rect, point) => {
                // The click and the hover accent resolve rows the same way, so
                // the row that lit up is the row that runs. A click on the box
                // but not on a row (the body, the border) changes nothing.
                let Some(row) = dialog.row_at(mouse.column, mouse.row) else {
                    return true;
                };
                if let Some(dialog) = self.confirm.as_mut() {
                    dialog.choices.selected = row;
                }
                self.confirm_accept();
                true
            },
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right) => {
                // Clicking away is the same answer as any unbound key: cancel.
                self.confirm_cancel();
                true
            },
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if let Some(dialog) = self.confirm.as_mut() {
                    dialog.set_hover(Some(point));
                }
                true
            },
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_without_a_label_falls_back_to_its_action() {
        assert_eq!(confirm_label(&ConfirmAction::Cancel), "Cancel");
        assert_eq!(
            confirm_label(&ConfirmAction::DiscardPaths(Vec::new())),
            "Discard"
        );
        assert_eq!(
            confirm_label(&ConfirmAction::DeleteExplorerPaths(Vec::new())),
            "Delete"
        );
        assert_eq!(
            confirm_label(&ConfirmAction::Command(Command::Quit)),
            Command::Quit.label()
        );
    }

    #[test]
    fn paths_are_named_relative_to_the_workspace_root() {
        let root = Path::new("/w");
        let paths = vec![PathBuf::from("/w/src/a.rs"), PathBuf::from("/w/b.rs")];
        assert_eq!(describe_paths(&paths, root), "src/a.rs, b.rs");
    }

    #[test]
    fn a_path_outside_the_root_keeps_its_full_name() {
        let paths = vec![PathBuf::from("/elsewhere/a.rs")];
        assert_eq!(describe_paths(&paths, Path::new("/w")), "/elsewhere/a.rs");
    }

    #[test]
    fn a_long_list_names_the_first_few_and_counts_the_rest() {
        let root = Path::new("/w");
        let paths: Vec<PathBuf> = (0..9)
            .map(|i| PathBuf::from(format!("/w/{i}.rs")))
            .collect();
        assert_eq!(
            describe_paths(&paths, root),
            "0.rs, 1.rs, 2.rs, 3.rs, 4.rs, 5.rs, and 3 more"
        );
    }

    #[test]
    fn a_list_exactly_at_the_cap_is_not_summarized() {
        let root = Path::new("/w");
        let paths: Vec<PathBuf> = (0..NAMED_PATHS)
            .map(|i| PathBuf::from(format!("/w/{i}.rs")))
            .collect();
        let described = describe_paths(&paths, root);
        assert!(!described.contains("more"), "{described}");
    }

    #[test]
    fn no_paths_describe_as_nothing() {
        assert_eq!(describe_paths(&[], Path::new("/w")), "");
    }
}
