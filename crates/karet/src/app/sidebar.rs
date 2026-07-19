use super::*;

impl App {
    /// Move focus between the sidebar and the editor.
    pub(super) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Editor,
            Focus::Editor => Focus::Sidebar,
            // Toggling out of the outline returns to the editor it annotates.
            Focus::Outline => Focus::Editor,
        };
    }

    /// The flattened outline rows for the active tab, or empty when it has none.
    pub(crate) fn active_outline_rows(&self) -> Vec<OutlineRow> {
        match self.tabs.get(self.active).map(|t| &t.kind) {
            #[cfg(feature = "pdf")]
            Some(TabKind::Document { outline, .. }) => {
                crate::outline::flatten(&crate::outline::from_pdf(outline))
            },
            _ => Vec::new(),
        }
    }

    /// Keep the outline row selection's length in step with the active tab's outline.
    pub(super) fn sync_outline_selection(&mut self) {
        let n = self.active_outline_rows().len();
        self.outline_sel.set_len(n);
    }

    /// Show or hide the right-side outline panel. Showing it focuses the panel (so it
    /// is navigable at once); hiding it returns focus to the editor.
    pub(super) fn toggle_outline(&mut self) {
        self.outline_visible = !self.outline_visible;
        if self.outline_visible {
            self.sync_outline_selection();
            // Focus the panel for immediate navigation, but only when it has content —
            // an empty "No outline" panel should not steal focus from the editor.
            if !self.active_outline_rows().is_empty() {
                self.focus = Focus::Outline;
            }
        } else if self.focus == Focus::Outline {
            self.focus = Focus::Editor;
        }
    }

    /// Move the outline selection by `delta` rows.
    pub(super) fn outline_step(&mut self, delta: i32) {
        self.sync_outline_selection();
        self.outline_sel.move_by(delta);
    }

    /// Leave the outline panel, returning focus to the editor (the panel stays open).
    pub(super) fn outline_collapse(&mut self) {
        self.focus = Focus::Editor;
    }

    /// Navigate to the selected outline entry: jump a document to its page, or move
    /// the editor caret to its position.
    pub(super) fn outline_activate(&mut self) {
        let rows = self.active_outline_rows();
        let Some(target) = rows.get(self.outline_sel.cursor()).and_then(|r| r.target) else {
            return;
        };
        match target {
            OutlineTarget::Page(page) => self.set_document_page(page),
            OutlineTarget::Text(pos) => {
                // Clone the buffer (O(1) rope share) so the editor borrow is free of the
                // tab-kind borrow.
                let buffer = match self.tabs.get(self.active).map(|t| &t.kind) {
                    Some(TabKind::Code { buffer, .. }) => Some(buffer.clone()),
                    _ => None,
                };
                if let (Some(buffer), Some(tab)) = (buffer, self.tabs.get_mut(self.active)) {
                    tab.editor.goto(&buffer, pos);
                }
            },
        }
    }

    /// Route a left-click in the outline panel: focus it, select the clicked row, and
    /// navigate to it. `outline_scroll` (recorded during draw) maps the screen row to
    /// the right entry even when the list is scrolled.
    pub(super) fn handle_outline_click(&mut self, row_y: u16) {
        self.focus = Focus::Outline;
        let top = self.outline_content_rect.y;
        if row_y < top {
            return; // a click on the header just focuses the panel
        }
        let rows = self.active_outline_rows();
        self.outline_sel.set_len(rows.len());
        let idx = self.outline_scroll + usize::from(row_y - top);
        if idx >= rows.len() {
            return;
        }
        self.outline_sel.move_to(idx);
        self.outline_activate();
    }

    /// Set the active document tab's current page (clamped to the page range). A
    /// no-op without the `pdf` feature, where no document tab exists.
    pub(super) fn set_document_page(&mut self, page: usize) {
        #[cfg(feature = "pdf")]
        if let Some(Tab {
            kind:
                TabKind::Document {
                    page: current,
                    page_count,
                    ..
                },
            ..
        }) = self.tabs.get_mut(self.active)
        {
            *current = page.min(page_count.saturating_sub(1));
        }
        #[cfg(not(feature = "pdf"))]
        let _ = page;
    }

    /// Route a mouse-wheel notch over the sidebar: the Source-Control panel scrolls
    /// its list (so the commit log is reachable), while the explorer and search move
    /// their selection one step per notch.
    pub(super) fn sidebar_wheel(&mut self, delta: i32, at_row: u16) {
        match self.sidebar_panel {
            // Route to whichever Source-Control region the pointer is over: the pinned
            // commit-log at the bottom, or the changes list above it.
            SidebarPanel::SourceControl => {
                if row_in_rect(self.scm_commits_rect, at_row) {
                    self.scm_scroll_commits(delta);
                } else {
                    self.scm_scroll_changes(delta);
                }
            },
            // The wheel only moves the selection — it must not open previews, or
            // scrolling past files would thrash the preview slot open with every
            // notch. Deliberate navigation (the arrow keys) goes through
            // [`sidebar_step`](Self::sidebar_step), which does preview.
            _ => self.sidebar_move(delta.signum()),
        }
    }

    /// Move the sidebar selection within the active panel, without opening
    /// anything (the wheel path; [`sidebar_step`](Self::sidebar_step) layers
    /// selection-follows-preview on top for keyboard navigation).
    pub(super) fn sidebar_move(&mut self, delta: i32) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                if delta > 0 {
                    self.explorer.select_next();
                } else {
                    self.explorer.select_prev();
                }
            },
            // A plain move collapses any range or multi-selection; the viewport then
            // follows the change cursor so it stays visible.
            SidebarPanel::SourceControl => {
                self.scm.selection.move_by(delta);
                self.scm_follow_cursor();
            },
            SidebarPanel::Search => self.search_select(delta),
        }
    }

    /// Move the sidebar selection one step by keyboard (arrows / `j`/`k`), then
    /// follow it with a preview: the landed-on file or change opens in the pane's
    /// preview slot *without* stealing focus, so navigation keeps flowing and the
    /// panel's own keys (e.g. staging) stay live.
    pub(super) fn sidebar_step(&mut self, delta: i32) {
        self.sidebar_move(delta);
        match self.sidebar_panel {
            // A directory row leaves the editor area untouched.
            SidebarPanel::Explorer => self.preview_selected_explorer_row(),
            SidebarPanel::SourceControl => self.preview_selected_diff(),
            SidebarPanel::Search => {},
        }
    }

    /// The display row of the Source-Control change cursor. Both section headers are
    /// always drawn, and an empty section reserves one placeholder line, so the
    /// staged block is `1` header + `max(staged, 1)` rows regardless of contents.
    pub(super) fn scm_cursor_display_row(&self) -> usize {
        let i = self.scm.selection.cursor();
        let staged = self.scm.staged_count;
        if i < staged {
            // In the staged section: the "STAGED CHANGES" header sits above it.
            1 + i
        } else {
            // In the working section: the full staged block plus the "CHANGES" header.
            let staged_block = 1 + staged.max(1);
            staged_block + 1 + (i - staged)
        }
    }

    /// Scroll the changes region so the change cursor stays visible.
    pub(super) fn scm_follow_cursor(&mut self) {
        let h = self.scm_changes_rect.height as usize;
        if h == 0 {
            return;
        }
        let row = self.scm_cursor_display_row();
        if row < self.scm_offset {
            self.scm_offset = row;
        } else if row >= self.scm_offset + h {
            self.scm_offset = row + 1 - h;
        }
    }

    /// Scroll the changes region by `delta` rows, clamped to its content.
    pub(super) fn scm_scroll_changes(&mut self, delta: i32) {
        let max = self
            .scm_total_rows
            .saturating_sub(self.scm_changes_rect.height as usize);
        let next = (self.scm_offset as i64 + i64::from(delta)).clamp(0, max as i64);
        self.scm_offset = next as usize;
    }

    /// Scroll the pinned commit-log region by `delta` rows, clamped to its content,
    /// and lazily load more commits near the bottom.
    pub(super) fn scm_scroll_commits(&mut self, delta: i32) {
        let max = self
            .scm_commits_total
            .saturating_sub(self.scm_commits_rect.height as usize);
        let next = (self.scm_commits_offset as i64 + i64::from(delta)).clamp(0, max as i64);
        self.scm_commits_offset = next as usize;
        self.maybe_autoload_commits();
    }

    /// Request the next commit page once the commit-log region nears the end of what
    /// is loaded.
    pub(super) fn maybe_autoload_commits(&mut self) {
        if !self.scm.log_has_more || self.scm.log_loading {
            return;
        }
        let bottom = self.scm_commits_offset + self.scm_commits_rect.height as usize;
        if bottom + COMMIT_AUTOLOAD_THRESHOLD >= self.scm_commits_total {
            self.load_more_scm_log();
        }
    }

    /// Activate the selected sidebar row — the explicit Enter "commit into the
    /// view" action: expand a dir, or open the file/diff *materialized* (never a
    /// preview) with keyboard focus moving into it. An already-open view — even
    /// the preview slot — is re-focused and made permanent instead of duplicated.
    /// Browsing (arrow moves) previews without stealing focus; a single click
    /// previews with focus (see [`handle_sidebar_click`](Self::handle_sidebar_click)).
    pub(super) fn sidebar_activate(&mut self) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => self.sidebar_promote_or_open_permanent(),
            SidebarPanel::SourceControl => self.open_selected_diff(),
            SidebarPanel::Search => {},
        }
    }

    /// Open the explorer's selected file in the pane's preview slot with keyboard
    /// focus moving to the editor — the single-click action (VS Code parity: a
    /// click previews and focuses; Enter / double-click materializes). A directory
    /// row toggles its expansion.
    pub(super) fn explorer_preview_with_focus(&mut self) {
        self.explorer.ensure_built(&self.root);
        if let Some(row) = self.explorer.selected() {
            let path = row.path.clone();
            if row.is_dir {
                self.explorer.toggle(&path);
            } else {
                self.open_path_preview(&path, true);
            }
        }
    }

    /// Open the explorer's selected row in the pane's preview slot without
    /// stealing keyboard focus (selection-follows-preview). A directory row (or an
    /// empty selection) changes nothing; a file already open is just shown. The
    /// sidebar keeps focus so the user can keep arrowing through the tree.
    pub(super) fn preview_selected_explorer_row(&mut self) {
        let Some(row) = self.explorer.selected() else {
            return;
        };
        if row.is_dir {
            return;
        }
        let path = row.path.clone();
        self.open_path_preview(&path, false);
    }

    /// Enter or double-click on a file in the tree: promote it to a permanent tab
    /// instead of the single-click preview behavior. If it's already open (as the
    /// preview tab or otherwise), just clears its preview flag in place; if not
    /// yet open, opens it as a new permanent tab via [`open_path`](Self::open_path).
    pub(super) fn sidebar_promote_or_open_permanent(&mut self) {
        if self.sidebar_panel != SidebarPanel::Explorer {
            return;
        }
        self.explorer.ensure_built(&self.root);
        let Some(row) = self.explorer.selected() else {
            return;
        };
        if row.is_dir {
            self.explorer.toggle(&row.path.clone());
            return;
        }
        let path = row.path.clone();
        let target = canonical(&path);
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| !t.is_diff() && t.path().is_some_and(|p| canonical(p) == target))
        {
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.is_preview = false;
            }
            self.select_tab(idx);
        } else {
            self.open_path(&path);
        }
    }

    /// Collapse the selected directory (explorer only).
    pub(super) fn sidebar_collapse(&mut self) {
        if self.sidebar_panel == SidebarPanel::Explorer
            && let Some(row) = self.explorer.selected()
            && row.is_dir
        {
            let path = row.path.clone();
            self.explorer.collapse(&path);
        }
    }

    /// Toggle expansion of the selected directory (explorer only).
    pub(super) fn sidebar_toggle_expand(&mut self) {
        if self.sidebar_panel == SidebarPanel::Explorer {
            self.explorer.toggle_selected();
        }
    }

    /// Reveal `path` in the Explorer sidebar (VS Code's "Reveal in Explorer"): show
    /// the Explorer panel, expand every ancestor directory within the workspace root
    /// (and the target itself when it is a directory), select the target's row,
    /// scroll it into view (the tree clamps its offset to the cursor on the next
    /// render), and move keyboard focus to the sidebar.
    ///
    /// A no-op — save a short status note — when `path` lies outside the workspace
    /// root or no longer maps to a row in the tree.
    pub(crate) fn reveal_in_explorer(&mut self, path: &Path) {
        if !path_under(&self.root, path) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("path");
            self.status = Some(format!("reveal: {name} is outside the workspace"));
            return;
        }
        // The workspace root itself has no row (the tree lists its children): just
        // show and focus the Explorer without disturbing the selection.
        if same_path(path, &self.root) {
            let root = self.root.clone();
            self.explorer.ensure_built(&root);
            self.sidebar_panel = SidebarPanel::Explorer;
            self.sidebar_visible = true;
            self.focus = Focus::Sidebar;
            return;
        }
        // Expand every ancestor directory from the root down to the target, plus the
        // target when it is a directory. Inserting every ancestor covers directory
        // chain compaction: the chain's tip is always among them, so a single rebuild
        // unfolds the whole path.
        let root = self.root.clone();
        for anc in path.ancestors() {
            if anc == path {
                continue;
            }
            if !path_under(&root, anc) {
                break;
            }
            self.explorer.expand(anc);
        }
        if path.is_dir() {
            self.explorer.expand(path);
        }
        self.explorer.ensure_built(&root);
        let Some(idx) = self.explorer_row_index(path) else {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("path");
            self.status = Some(format!("reveal: {name} is not in the explorer"));
            return;
        };
        self.explorer.select_index(idx);
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
    }

    /// The explorer row index for `path`: an exact row match (files, plain
    /// directories, and directory-chain tips), else the shallowest compacted chain
    /// row whose tip lies within `path` (a directory folded into an `a/b` row).
    pub(super) fn explorer_row_index(&self, path: &Path) -> Option<usize> {
        let rows = self.explorer.rows();
        if let Some(idx) = rows.iter().position(|row| row.path == path) {
            return Some(idx);
        }
        rows.iter()
            .enumerate()
            .filter(|(_, row)| row.is_dir && row.path.starts_with(path))
            .min_by_key(|(_, row)| row.path.components().count())
            .map(|(idx, _)| idx)
    }
}
