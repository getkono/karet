use super::*;

impl App {
    /// Apply a resolved [`Command`].
    pub(super) fn dispatch(&mut self, command: Command) {
        match command {
            Command::Quit => self.request_quit(),
            Command::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            Command::ToggleFocus => self.toggle_focus(),
            Command::SelectPanel(panel) => {
                self.sidebar_panel = panel;
                self.sidebar_visible = true;
                self.focus = Focus::Sidebar;
                // Lazily fetch the first commit-log page when Source Control opens.
                if panel == SidebarPanel::SourceControl && self.scm.log.is_empty() {
                    self.request_scm_log(0);
                }
                if panel == SidebarPanel::SourceControl {
                    self.request_repository_snapshot();
                }
            },
            Command::OpenQuickOpen => self.open_quick_open(),
            Command::OpenCommandPalette => self.overlay = Some(Overlay::command_palette()),
            Command::OpenFind => self.open_find(),
            Command::OpenGlobalSearch => self.start_global_search(),
            Command::CloseTab => self.request_close_active_tab(),
            Command::NextTab => self.next_tab(),
            Command::PrevTab => self.prev_tab(),
            Command::MoveTabLeft => self.move_active_tab(-1),
            Command::MoveTabRight => self.move_active_tab(1),
            Command::GoToTab(n) => self.go_to_tab(n),
            Command::CloseOtherTabs => self.guarded_close(CloseRequest::OtherTabs),
            Command::CloseTabsToRight => self.guarded_close(CloseRequest::TabsToRight),
            Command::CloseAllTabs => self.guarded_close(CloseRequest::AllTabs),
            Command::ReopenClosedTab => self.reopen_closed_tab(),
            Command::OpenAnyway => self.open_active_anyway(),
            Command::DismissNotification => self.notifications.dismiss_latest(),
            Command::DismissAllNotifications => self.notifications.dismiss_all(),
            Command::MarkdownPreviewSide => self.open_markdown_preview_side(),
            Command::SplitRight => self.split_focused(SplitDir::Right),
            Command::SplitDown => self.split_focused(SplitDir::Down),
            Command::FocusNextPane => self.focus_pane_cycle(true),
            Command::FocusPrevPane => self.focus_pane_cycle(false),
            Command::Copy => self.copy_selection(),
            Command::CopyPath => self.copy_path(false),
            Command::CopyRelativePath => self.copy_path(true),
            Command::RevealActiveInExplorer => self.reveal_active_in_explorer(),
            Command::CopyRemoteFileUrl => self.copy_remote_link(remote::LinkKind::RemoteFile),
            Command::CopyGithubPermalink => {
                self.copy_remote_link(remote::LinkKind::GithubPermalink);
            },
            Command::CopyGithubHeadLink => {
                self.copy_remote_link(remote::LinkKind::GithubHeadLink);
            },
            Command::OpenChangesWithPrevious => self.open_changes_with("HEAD", "HEAD"),
            Command::OpenChangesWithRevision => self.open_changes_pick_revision(),
            Command::OpenChangesWithBranch => self.open_changes_pick_branch(),
            Command::SidebarUp => self.sidebar_step(-1),
            Command::SidebarDown => self.sidebar_step(1),
            Command::SidebarActivate => self.sidebar_activate(),
            Command::SidebarCollapse => self.sidebar_collapse(),
            Command::SidebarToggleExpand => self.sidebar_toggle_expand(),
            Command::ToggleOutline => self.toggle_outline(),
            Command::OutlineUp => self.outline_step(-1),
            Command::OutlineDown => self.outline_step(1),
            Command::OutlineActivate => self.outline_activate(),
            Command::OutlineCollapse => self.outline_collapse(),
            Command::CaretUp => self.caret_motion(false, EditorState::move_up),
            Command::CaretDown => self.caret_motion(false, EditorState::move_down),
            Command::CaretLeft => self.caret_motion(false, EditorState::move_left),
            Command::CaretRight => self.caret_motion(false, EditorState::move_right),
            Command::SelectUp => self.caret_motion(true, EditorState::move_up),
            Command::SelectDown => self.caret_motion(true, EditorState::move_down),
            Command::SelectLeft => self.caret_motion(true, EditorState::move_left),
            Command::SelectRight => self.caret_motion(true, EditorState::move_right),
            Command::CaretWordLeft => self.caret_motion(false, EditorState::move_word_left),
            Command::CaretWordRight => self.caret_motion(false, EditorState::move_word_right),
            Command::CaretLineStart => self.caret_motion(false, EditorState::move_line_start),
            Command::CaretLineEnd => self.caret_motion(false, EditorState::move_line_end),
            Command::CaretDocStart => self.caret_motion(false, EditorState::move_doc_start),
            Command::CaretDocEnd => self.caret_motion(false, EditorState::move_doc_end),
            Command::SelectWordLeft => self.caret_motion(true, EditorState::move_word_left),
            Command::SelectWordRight => self.caret_motion(true, EditorState::move_word_right),
            Command::SelectLineStart => self.caret_motion(true, EditorState::move_line_start),
            Command::SelectLineEnd => self.caret_motion(true, EditorState::move_line_end),
            Command::SelectDocStart => self.caret_motion(true, EditorState::move_doc_start),
            Command::SelectDocEnd => self.caret_motion(true, EditorState::move_doc_end),
            Command::SelectPageUp => self.caret_motion(true, EditorState::page_up),
            Command::SelectPageDown => self.caret_motion(true, EditorState::page_down),
            Command::EditorSelectAll => self.editor_select_all(),
            Command::AddCursorAbove => self.add_cursor_vertical(true),
            Command::AddCursorBelow => self.add_cursor_vertical(false),
            Command::AddCursorNextOccurrence => self.add_cursor_next_occurrence(),
            Command::CollapseCarets => self.collapse_carets_or_unfocus(),
            Command::ScrollUp => self.scroll_lines(-1),
            Command::ScrollDown => self.scroll_lines(1),
            Command::PageUp => self.scroll_lines(-i32::from(self.main_rect.height.max(1))),
            Command::PageDown => self.scroll_lines(i32::from(self.main_rect.height.max(1))),
            Command::Top => self.scroll_edge(true),
            Command::Bottom => self.scroll_edge(false),
            Command::ToggleDiffLayout => self.toggle_diff_layout(),
            Command::ToggleFold => self.toggle_fold(),
            Command::NextChangedFile => self.step_changed_file(1),
            Command::PrevChangedFile => self.step_changed_file(-1),
            Command::OpenDiffFile => self.open_diff_file(),
            Command::TriggerCompletion => self.trigger_completion(true),
            Command::InsertChar(c) => {
                let s = c.to_string();
                self.submit_edit_with_cause(EditCause::Type, move |caret, sel, _b, base| {
                    Some(editing::insert(caret, sel, base, &s))
                });
                self.maybe_auto_complete(c);
            },
            Command::InsertNewline => {
                self.submit_edit_with_cause(EditCause::Newline, |caret, sel, buf, base| {
                    Some(editing::newline(caret, sel, buf, base))
                });
            },
            Command::DeleteBackward => {
                self.submit_edit_with_cause(EditCause::Delete, editing::backspace)
            },
            Command::DeleteForward => {
                self.submit_edit_with_cause(EditCause::Delete, editing::delete_forward);
            },
            Command::Indent => {
                self.submit_edit(|caret, sel, _b, base| Some(editing::indent(caret, sel, base)));
            },
            Command::Dedent => {
                self.submit_edit(|caret, _sel, buf, base| editing::dedent(caret, buf, base));
            },
            Command::Undo => self.send_doc_command(|doc| SessionCommand::Undo { doc }),
            Command::Redo => self.send_doc_command(|doc| SessionCommand::Redo { doc }),
            Command::Save => self.save_active(),
            Command::Cut => self.cut(),
            Command::Paste => self.paste_from_clipboard(),
            Command::SelectExtendUp => self.sidebar_select_extend(-1),
            Command::SelectExtendDown => self.sidebar_select_extend(1),
            Command::SelectToggle => self.sidebar_select_toggle(),
            Command::SelectAll => self.sidebar_select_all(),
            Command::ScmStage => self.scm_send_paths(|paths| SessionCommand::Stage { paths }),
            Command::ScmUnstage => self.scm_send_paths(|paths| SessionCommand::Unstage { paths }),
            Command::ScmToggleStage => self.scm_toggle_stage(),
            Command::ScmStageAll => self.send_vcs(SessionCommand::StageAll),
            Command::ScmUnstageAll => self.send_vcs(SessionCommand::UnstageAll),
            Command::ScmDiscard => self.scm_arm_discard(),
            Command::ScmCommit => self.scm_open_commit_input(),
            Command::ScmRefresh => {
                self.send_vcs(SessionCommand::RefreshVcs);
                self.request_repository_snapshot();
            },
            Command::ScmSync => self.run_vcs_action(VcsAction::Sync),
            Command::ScmMenu => self.open_scm_menu(),
            Command::ScmSwitchBranch => self.open_branch_picker(),
            Command::ScmCreateBranch => self.open_create_branch_form(),
            Command::ScmPickPullRequest => self.open_pull_request_picker(),
            Command::ScmUndoCommit => {
                self.run_vcs_action(VcsAction::UndoCommit {
                    allow_upstream: false,
                });
            },
            Command::ScmStash => self.open_stash_form(),
            Command::ScmManageStashes => self.open_stash_manager(),
            Command::ScmPublish => self.publish_current_branch(),
            Command::ScmRenameBranch => self.prompt_rename_current_branch(),
            Command::ScmDeleteBranch => self.open_delete_branch_picker(),
            Command::ScmDeleteRemoteBranch => self.open_delete_remote_branch_picker(),
            Command::ScmContinue => self.run_vcs_action(VcsAction::Continue),
            Command::ScmAbort => self.run_vcs_action(VcsAction::Abort),
            Command::ScmSkip => self.run_vcs_action(VcsAction::Skip),
            Command::ToggleInlineBlame => self.toggle_live_blame(),
            Command::OpenBlameDetail => self.open_live_blame_detail(),
            Command::ShowLoadedConfig => {
                if self.backend.is_some() {
                    self.send_command(SessionCommand::LoadedConfig);
                } else {
                    self.open_loaded_config(self.loaded_config.clone());
                }
            },
            Command::ExplorerNewFile => self.explorer_begin_new(false),
            Command::ExplorerNewFolder => self.explorer_begin_new(true),
            Command::ExplorerRename => self.explorer_begin_rename(),
            Command::ExplorerRefresh => self.explorer_refresh(),
            Command::ExplorerCollapseAll => self.explorer.collapse_all(),
            Command::ExplorerCopy => self.explorer_copy_files(),
            Command::ExplorerCut => self.explorer_cut_files(),
            Command::ExplorerPaste => self.explorer_paste_files(),
            Command::ExplorerDuplicate => self.explorer_duplicate_files(),
            Command::ExplorerDelete => self.explorer_arm_delete(),
            Command::ExplorerCopyPath => self.explorer_copy_path(false),
            Command::ExplorerCopyRelativePath => self.explorer_copy_path(true),
            Command::ExplorerOpenContextMenu => self.open_context_menu_for_selection(),

            // Modal-scoped commands (resolved only while a modal context is active).
            Command::OverlayUp => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.select_up();
                }
            },
            Command::OverlayDown => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.select_down();
                }
            },
            Command::OverlayAccept => self.overlay_accept(),
            Command::OverlayCancel => self.overlay = None,
            Command::FindNext => self.find_step(1),
            Command::FindPrev => self.find_step(-1),
            Command::FindCancel => self.close_find(),
            Command::FindSubmit => self.find_submit(),
            Command::FindReplaceAll => self.find_replace_all(),
            Command::FindToggleReplace => self.find_toggle_replace(),
            Command::FindToggleField => self.find_toggle_field(),
            Command::FindToggleRegex => self.find_toggle_option(SearchOption::Regex),
            Command::FindToggleCase => self.find_toggle_option(SearchOption::Case),
            Command::FindToggleWord => self.find_toggle_option(SearchOption::Word),
            Command::CommitSubmit => self.commit_submit(),
            Command::CommitCancel => self.commit_cancel(),
            Command::CommitGenerate => self.commit_generate(),
            Command::ExplorerEditSubmit => self.explorer_commit_edit(),
            Command::ExplorerEditCancel => self.explorer.cancel_edit(),
            Command::ConfirmDiscard => self.resolve_discard(true),
            Command::ConfirmExplorerDelete => self.resolve_explorer_delete(true),
            Command::ContextMenuUp => self.context_menu_step(-1),
            Command::ContextMenuDown => self.context_menu_step(1),
            Command::ContextMenuAccept => self.accept_context_menu(),
            Command::ContextMenuCancel => self.close_context_menu(),
            Command::CloseConfirmSave => self.close_save(),
            Command::CloseConfirmDiscard => self.close_discard(),
            Command::RecoverSwaps => {
                // Open a tab for each backed-up file first (so the recovered content
                // has somewhere to land), then ask the backend to restore the buffers.
                if let Some(swaps) = self.pending_swaps.take() {
                    for info in &swaps {
                        self.open_path(&info.original);
                    }
                    self.status = Some(format!("recovering {} file(s)…", swaps.len()));
                }
                self.send_command(SessionCommand::RecoverSwaps);
            },
            Command::DiscardSwaps => {
                self.pending_swaps = None;
                self.send_command(SessionCommand::DiscardSwaps);
            },
            Command::ShowDependencyGraph => {
                self.status = Some("building dependency graph…".to_string());
                self.send_command(SessionCommand::DependencyGraph);
            },
            Command::ShowCommitGraph => self.open_commit_graph(),
            Command::CommitGraphNext => self.graph_select(1),
            Command::CommitGraphPrev => self.graph_select(-1),
            Command::CommitGraphOpen => self.graph_open_selected(),
            Command::OpenCommitByHash => self.open_rev_input(),
            Command::RevInputSubmit => self.rev_submit(),
            Command::RevInputCancel => self.rev_cancel(),
            Command::ShowFileHistory => self.open_file_history(),
            Command::DiffUnpushed => self.open_range(SessionCommand::RangeChanges {
                spec: RangeSpec::Unpushed,
            }),
            Command::DiffSinceBase => self.open_range(SessionCommand::RangeChanges {
                spec: RangeSpec::SinceBase { base: None },
            }),
            Command::CommitGraphMarkBase => self.graph_mark_base(),
            Command::CommitGraphCompare => self.graph_compare(),
            Command::SearchSelectUp => self.search_select(-1),
            Command::SearchSelectDown => self.search_select(1),
            Command::SearchOpen => self.open_selected_result(),
            Command::SearchBeginInput => self.search.input = true,
            Command::SearchQuit => self.should_quit = true,
            Command::SearchRun => self.run_search_query(),
            Command::SearchEndInput => self.search.input = false,
            Command::SearchToggleReplace => self.search_toggle_replace(),
            Command::SearchToggleField => self.search_toggle_field(),
            Command::SearchReplaceAll => self.search_replace_all(),
            Command::SearchToggleRegex => self.search_toggle_regex(),
            Command::SearchToggleCase => self.search_toggle_case(),
            Command::SearchToggleWord => self.search_toggle_word(),
        }
    }
}
