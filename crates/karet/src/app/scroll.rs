//! Horizontal scrolling, and the absolute scrolling a scrollbar drives.
//!
//! The wheel and the keyboard move by a *delta*; a scrollbar hands over a *position*.
//! Those are not the same operation. Three views throw a delta's magnitude away (the
//! commit browser and the sidebar lists move a selection by one, a document turns one
//! page), and stepping the editor there would re-wrap the document once per row. So
//! [`App::scroll_surface_to`] is its own path rather than a wrapper over
//! [`App::scroll_lines`].
//!
//! It divides the views in two. Most own an offset the renderer honours, and setting
//! it is the whole job. The rest derive their offset from a cursor *during* the
//! render — the file tree, the search and spelling lists, the outline, the commit
//! browser, the GitHub dashboard — and for those an offset written on its own is
//! undone before it is ever seen. Scrolling them means moving the cursor into the
//! window the pointer asked for, which is also what their wheel already does.

use super::*;

/// A cursor pulled into the window `[position, position + viewport)`, left alone when
/// it is already inside it.
pub(in crate::app) fn cursor_in_window(
    cursor: usize,
    position: usize,
    viewport: usize,
    len: usize,
) -> usize {
    if viewport == 0 || len == 0 {
        return cursor;
    }
    let bottom = position.saturating_add(viewport - 1).min(len - 1);
    cursor.clamp(position.min(bottom), bottom)
}

impl App {
    /// Scroll the active non-wrapping view horizontally by `delta` columns.
    pub(super) fn scroll_columns(&mut self, delta: i32) {
        let word_wrap = self.tabs.get(self.active).is_some_and(|tab| {
            effective_word_wrap(
                tab,
                self.settings
                    .editor
                    .for_language(tab_language(tab))
                    .word_wrap(),
            )
        });
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            TabKind::Code { buffer, .. } if !word_wrap => {
                tab.editor.scroll_columns(buffer, delta);
            },
            TabKind::Diff { pager, .. }
            | TabKind::StashPreview { pager, .. }
            | TabKind::Graph { pager, .. }
            | TabKind::LoadedConfig { pager, .. }
            | TabKind::CommitLoading { pager, .. } => adjust(&mut pager.column, delta),
            TabKind::CommitGraph { column, .. } => adjust(column, delta),
            TabKind::Commit { view, .. } | TabKind::Compare { view, .. } => {
                adjust(&mut view.column, delta);
            },
            _ => {},
        }
    }

    /// Route a mouse event that landed on a scrollbar track.
    ///
    /// Returns `true` when the pointer was on a track, so the event is consumed even
    /// where the gesture does nothing — the column belongs to the bar, and a press
    /// there must never fall through to the view beside it.
    pub(super) fn scrollbar_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(hit) = self.scroll_hits.at(mouse.column, mouse.row) else {
            return false;
        };
        let extent = hit.track.extent();
        match mouse.kind {
            // One unit per notch, against the three the content area moves. A track
            // cell stands for many lines on a long file, and a proportional drag
            // cannot address the lines in between, so the bar is where the fine
            // control lives. Going through the absolute setter is what makes this a
            // single unit even on the views whose wheel handlers throw the magnitude
            // away — the commit browser, the sidebar lists, a document's pages.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollRight => {
                self.scroll_surface_to(hit.surface, extent.step(1), extent.viewport);
            },
            MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft => {
                self.scroll_surface_to(hit.surface, extent.step(-1), extent.viewport);
            },
            MouseEventKind::Down(MouseButton::Left) => {
                match hit.track.hit(mouse.column, mouse.row) {
                    Some(TrackHit::Thumb) => {
                        self.scroll_drag = Some(ScrollDrag {
                            surface: hit.surface,
                            track: hit.track,
                            origin_cell: hit.track.along(mouse.column, mouse.row),
                            origin_position: extent.position,
                        });
                    },
                    Some(TrackHit::Before) => {
                        self.scroll_surface_to(hit.surface, extent.page_back(), extent.viewport);
                    },
                    Some(TrackHit::After) => {
                        self.scroll_surface_to(hit.surface, extent.page_forward(), extent.viewport);
                    },
                    None => {},
                }
            },
            _ => {},
        }
        true
    }

    /// Apply a thumb drag: the position it was grabbed at, moved by how far the
    /// pointer has travelled along the track since.
    pub(super) fn scroll_drag_to(&mut self, drag: ScrollDrag, column: u16, row: u16) {
        let cells = i32::from(drag.track.along(column, row)) - i32::from(drag.origin_cell);
        let extent = drag.track.extent();
        let position = extent.position_after_drag(drag.track.length(), drag.origin_position, cells);
        self.scroll_surface_to(drag.surface, position, extent.viewport);
    }

    /// Put `surface`'s first visible unit at `position`, in that surface's own unit.
    ///
    /// `viewport` comes from the track that was grabbed, so a surface measured in
    /// something other than terminal rows — the GitHub dashboard counts items several
    /// rows tall, the language-servers inventory counts servers — keeps its own scale
    /// without this having to re-derive it.
    pub(super) fn scroll_surface_to(
        &mut self,
        surface: ScrollSurface,
        position: usize,
        viewport: usize,
    ) {
        match surface {
            ScrollSurface::TabRows => self.scroll_tab_rows_to(position, viewport),
            ScrollSurface::TabColumns => self.scroll_tab_columns_to(position),
            ScrollSurface::EditorPreview => self.set_markdown_preview_scroll(position),
            ScrollSurface::GithubPage => self.scroll_github_page_to(position, viewport),
            ScrollSurface::GithubPullRequestCommits => {
                if let Some(TabKind::Github(view)) =
                    self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
                {
                    view.scroll_commits_to(position);
                }
            },
            // The tree's offset is pinned to its cursor by the render, so the cursor
            // travels with the window.
            ScrollSurface::Explorer => self.explorer.scroll_to(position, viewport),
            ScrollSurface::Outline => {
                let len = self.outline.sel.len();
                self.outline.scroll = position;
                let cursor = cursor_in_window(self.outline.sel.cursor(), position, viewport, len);
                self.outline.sel.move_to(cursor);
            },
            // Search and spelling keep no offset at all: their list is rebuilt from a
            // fresh `ListState` every frame, so the window *is* a function of the
            // cursor. Selecting the row that would sit at the bottom of the wanted
            // window is what makes ratatui's minimal scroll land the top of it on
            // `position`.
            ScrollSurface::SearchResults => {
                let len = self.search.results.len();
                let cursor = window_bottom(position, viewport, len);
                self.search.selection.move_to(cursor);
            },
            ScrollSurface::SpellingResults => {
                let len = self.spelling.rows.len();
                let cursor = window_bottom(position, viewport, len);
                self.spelling.selection.move_to(cursor);
            },
            ScrollSurface::TodoResults => {
                let len = self.todos.rows.len();
                let cursor = window_bottom(position, viewport, len);
                self.todos.selection.move_to(cursor);
            },
            ScrollSurface::DebugResults => {
                let len = self.debug_panel.rows.len();
                let cursor = window_bottom(position, viewport, len);
                self.debug_panel.selection.move_to(cursor);
            },
            // Routed as a delta so the commit log keeps its lazy-loading trigger.
            ScrollSurface::ScmChanges => {
                self.scm_scroll_changes(delta_to(self.scm_ui.offset, position));
            },
            ScrollSurface::ScmCommits => {
                self.scm_scroll_commits(delta_to(self.scm_ui.commits_offset, position));
            },
            ScrollSurface::Completion => {
                if let Some(ui) = self.completion.as_mut() {
                    ui.list.scroll_to(position, viewport);
                }
            },
        }
    }

    /// Land the GitHub page on an absolute offset.
    ///
    /// Separate from [`scroll_tab_rows_to`](Self::scroll_tab_rows_to) because the
    /// GitHub surface is not a tab: routing its scrollbar through the focused tab
    /// would drag whatever document sits behind it.
    fn scroll_github_page_to(&mut self, position: usize, viewport: usize) {
        if let Some(TabKind::Github(view)) = self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            view.scroll_to(position, viewport);
        }
    }

    /// The absolute counterpart to [`scroll_lines`](Self::scroll_lines): the same
    /// per-tab-kind split, but landing on a position instead of stepping by a delta.
    fn scroll_tab_rows_to(&mut self, position: usize, viewport: usize) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            // Buffer lines, the unit `visible_lines()` reports the extent in.
            TabKind::Code { .. } => tab.editor.scroll_to_line(clamp_u32(position)),
            TabKind::MarkdownPreview { scroll, .. } => *scroll = clamp_u16(position),
            TabKind::Diff { pager, .. }
            | TabKind::StashPreview { pager, .. }
            | TabKind::Graph { pager, .. }
            | TabKind::LoadedConfig { pager, .. }
            | TabKind::CommitLoading { pager, .. } => pager.scroll = clamp_u16(position),
            TabKind::Commit { view, .. } | TabKind::Compare { view, .. } => {
                view.scroll = clamp_u16(position);
            },
            TabKind::Hex { scroll, .. } => *scroll = position,
            TabKind::LanguageServers(view) => view.offset = position,
            TabKind::Github(view) => view.scroll_to(position, viewport),
            // The graph view pans freely: dragging its scrollbar moves the viewport and
            // leaves the selection where the user put it.
            TabKind::CommitGraph { .. } => self.graph_scroll_to(position),
            #[cfg(feature = "pdf")]
            TabKind::Document {
                page, page_count, ..
            } => *page = position.min(page_count.saturating_sub(1)),
            _ => {},
        }
    }

    /// The absolute counterpart to [`scroll_columns`](Self::scroll_columns).
    fn scroll_tab_columns_to(&mut self, position: usize) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            TabKind::Code { .. } => tab.editor.scroll_to_column(clamp_u32(position)),
            TabKind::Diff { pager, .. }
            | TabKind::StashPreview { pager, .. }
            | TabKind::Graph { pager, .. }
            | TabKind::LoadedConfig { pager, .. }
            | TabKind::CommitLoading { pager, .. } => pager.column = clamp_u16(position),
            TabKind::CommitGraph { column, .. } => *column = clamp_u16(position),
            TabKind::Commit { view, .. } | TabKind::Compare { view, .. } => {
                view.column = clamp_u16(position);
            },
            _ => {},
        }
    }
}

/// The row that must be selected for a minimal-scroll list to settle its window's
/// top on `position` — the bottom row of that window.
fn window_bottom(position: usize, viewport: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    position
        .saturating_add(viewport.saturating_sub(1))
        .min(len - 1)
}

/// The signed step from `from` to `to`, for the arms that route through an existing
/// delta-taking scroll so its side effects (lazy loading) still run.
fn delta_to(from: usize, to: usize) -> i32 {
    let from = i64::try_from(from).unwrap_or(i64::MAX);
    let to = i64::try_from(to).unwrap_or(i64::MAX);
    i32::try_from(to - from).unwrap_or(if to > from { i32::MAX } else { i32::MIN })
}

/// A position narrowed to the `u16` several views store their offset in.
pub(in crate::app) fn clamp_u16(position: usize) -> u16 {
    u16::try_from(position).unwrap_or(u16::MAX)
}

/// A position narrowed to the `u32` the editor stores its offsets in.
fn clamp_u32(position: usize) -> u32 {
    u32::try_from(position).unwrap_or(u32::MAX)
}

fn adjust(offset: &mut u16, delta: i32) {
    let next = (i64::from(*offset) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
    *offset = next as u16;
}
