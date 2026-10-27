use std::io;
use std::io::Write;

use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use karet_widgets::SplitAxis;
use karet_widgets::scroll::TrackHit;

use crate::app::App;
use crate::app::MIN_SCM_REGION;
use crate::app::SIDEBAR_MIN_WIDTH;
use crate::app::hit::PaneResize;
use crate::app::hit::TextFieldTarget;
use crate::app::search::SearchPanelField;
use crate::app::search::SearchRow;
use crate::app::util::rect_contains;
use crate::command::Command;
use crate::keymap::Focus;
use crate::keymap::SidebarPanel;
use crate::view::View;

pub(super) mod chrome;
mod focus;

impl App {
    /// Resize the sidebar so its right edge sits at column `col`. Dragging narrower
    /// than [`SIDEBAR_MIN_WIDTH`] is read as intent to collapse. The responsive upper
    /// bound (terminal width) is applied when the layout is next computed.
    pub(super) fn resize_sidebar_to(&mut self, col: u16) {
        let width = col.saturating_sub(self.last_frame.sidebar_rect.x);
        if width < SIDEBAR_MIN_WIDTH {
            self.sidebar_visible = false;
            self.sidebar_resizing = false;
        } else {
            self.sidebar_width = width;
        }
    }

    /// Resize the pinned Source-Control commit-log region so its top edge (the drag
    /// divider) sits at `row`. The list area's bottom is fixed, so the commit region
    /// grows as the divider moves up; both regions keep at least [`MIN_SCM_REGION`].
    pub(super) fn resize_scm_commits_to(&mut self, row: u16) {
        let content = self.last_frame.sidebar_content_rect;
        let bottom = content.y + content.height;
        let list_top = self.scm_ui.changes_rect.y;
        let total = bottom.saturating_sub(list_top);
        // Reserve MIN for the changes region plus the 1-row divider.
        let max_commits = total.saturating_sub(MIN_SCM_REGION + 1).max(MIN_SCM_REGION);
        let h = bottom.saturating_sub(row).saturating_sub(1);
        self.scm_ui.commits_h = h.clamp(MIN_SCM_REGION, max_commits);
    }

    /// Hint the terminal's mouse pointer shape (OSC 22) over a draggable
    /// divider — `col-resize` over the sidebar-width divider, `row-resize` over
    /// the Source-Control commit-log divider, the default shape everywhere
    /// else — mirroring how a GUI editor shows a resize cursor on hover. A
    /// complete no-op when the terminal wasn't confirmed to support it at
    /// startup, and only writes when the shape actually changes (never spams
    /// an escape sequence per mouse-move event).
    pub(super) fn update_pointer_shape_hint(&mut self, mouse: &MouseEvent) {
        if !self.caps.pointer_shapes {
            return;
        }
        let over_seam_affordance = self.seam_affordance_at(mouse.column, mouse.row);
        let hits = &self.last_frame;
        let point = (mouse.column, mouse.row);
        let over_sidebar_divider = self.sidebar_resizing
            || (self.sidebar_visible && mouse.column == hits.sidebar_divider_x);
        let over_scm_divider = self.scm_ui.resizing
            || (self.sidebar_panel == SidebarPanel::SourceControl
                && self.scm_ui.divider_y != 0
                && mouse.row == self.scm_ui.divider_y
                && rect_contains(hits.sidebar_rect, point));
        let pane_axis = self
            .pane_resize
            .map(|resize| resize.divider.axis)
            .or_else(|| {
                hits.pane_dividers
                    .iter()
                    .find(|divider| divider.contains(point.0, point.1))
                    .map(|divider| divider.axis)
            });
        let over_blame = hits
            .blame_rect()
            .is_some_and(|rect| rect_contains(rect, point));
        let over_markdown_link = hits
            .markdown_link_hits()
            .any(|hit| rect_contains(hit.rect, point));
        let over_commits_title = self.sidebar_panel == SidebarPanel::SourceControl
            && rect_contains(self.scm_ui.commits_title_rect, point);
        // A thumb is draggable, and this shell hints every draggable it has. The grab
        // shape outlives the pointer leaving the track, because the drag does too.
        let thumb = hits
            .scroll
            .at(point.0, point.1)
            .and_then(|hit| hit.track.hit(point.0, point.1));
        let over_thumb = thumb == Some(TrackHit::Thumb);
        let shape = if self.scroll_drag.is_some() {
            Some("grabbing")
        } else if over_sidebar_divider || pane_axis == Some(SplitAxis::Cols) {
            Some("col-resize")
        } else if over_scm_divider || pane_axis == Some(SplitAxis::Rows) {
            Some("row-resize")
        } else if over_thumb {
            Some("grab")
        } else if over_blame
            || over_markdown_link
            || over_seam_affordance
            || over_commits_title
            || self.definition_hover.is_some()
        {
            Some("pointer")
        } else {
            None
        };
        if shape == self.caps.pointer_shape {
            return;
        }
        let _ = write!(io::stdout(), "\x1b]22;{}\x1b\\", shape.unwrap_or("default"));
        let _ = io::stdout().flush();
        self.caps.pointer_shape = shape;
    }

    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent) {
        self.handle_mouse_event(mouse);
        // Mouse clicks, drag-selection, tab switches, and pane focus changes can all
        // move the active caret without passing through the keyboard input hook.
        self.reconcile_completion();
        self.reconcile_hover();
        self.reconcile_rename();
        self.request_live_blame();
    }

    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        self.update_definition_hover(&mouse);
        self.update_inlay_hover(&mouse);
        self.update_pointer_shape_hint(&mouse);
        // Ahead of every surface that could swallow it: a release that an overlay
        // or a toast took would otherwise leave the drag running behind it.
        if self.end_editor_drag_on(&mouse) {
            return;
        }
        // Toasts float above everything (including the overlay), so hit-test them
        // first: left-click dismisses and right-click copies an error.
        if self.handle_toast_mouse(mouse) {
            return;
        }
        // A confirmation outranks the overlay it may have opened over, so it is
        // hit-tested before the overlay's blanket swallow.
        if self.handle_confirm_mouse(mouse) {
            return;
        }
        if self.overlay.is_some() {
            return;
        }
        if self.handle_diagnostic_view_mouse(mouse) {
            return;
        }
        // A click anywhere settles the rename prompt as Esc would, then does
        // what it would have done without it. A click outside the hover popup
        // closes it the same way.
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.rename = None;
        }
        self.dismiss_hover_on_press_outside(mouse);
        if self.handle_context_menu_mouse(mouse) {
            return;
        }
        // An in-progress tab drag captures motion until the button is released.
        if self.tab_drag.is_some() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.drag_tab_update(mouse.column, mouse.row);
                },
                MouseEventKind::Up(MouseButton::Left) => self.drag_tab_drop(),
                _ => {},
            }
            return;
        }
        // An in-progress sidebar resize captures motion until the button is released.
        if self.sidebar_resizing {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_sidebar_to(mouse.column),
                MouseEventKind::Up(MouseButton::Left) => self.sidebar_resizing = false,
                _ => {},
            }
            return;
        }
        // An in-progress Source-Control commit-divider resize captures motion likewise.
        if self.scm_ui.resizing {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_scm_commits_to(mouse.row),
                MouseEventKind::Up(MouseButton::Left) => self.scm_ui.resizing = false,
                _ => {},
            }
            return;
        }
        if let Some(resize) = self.pane_resize {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    let coordinate = match resize.divider.axis {
                        SplitAxis::Cols => mouse.column,
                        SplitAxis::Rows => mouse.row,
                    };
                    let current = self
                        .layout
                        .dividers(self.last_frame.main_rect)
                        .into_iter()
                        .find(|divider| {
                            divider.axis == resize.divider.axis
                                && divider.before == resize.divider.before
                                && divider.after == resize.divider.after
                        })
                        .map_or(resize.divider.position, |divider| divider.position);
                    let delta = i32::from(coordinate) - i32::from(current);
                    let delta = i16::try_from(delta).unwrap_or_else(|_| {
                        if delta.is_negative() {
                            i16::MIN
                        } else {
                            i16::MAX
                        }
                    });
                    if delta != 0 {
                        self.layout.resize_divider(
                            resize.divider,
                            delta,
                            self.last_frame.main_rect,
                        );
                    }
                },
                MouseEventKind::Up(MouseButton::Left) => self.pane_resize = None,
                _ => {},
            }
            return;
        }
        // A thumb drag captures the pointer until release, so it keeps scrolling when
        // the pointer strays off a one-column track — the standard scrollbar contract.
        if let Some(drag) = self.scroll_drag {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.scroll_drag_to(drag, mouse.column, mouse.row);
                },
                MouseEventKind::Up(MouseButton::Left) => self.scroll_drag = None,
                _ => {},
            }
            return;
        }
        // Lightweight Search/commit fields capture selection drags independently of
        // the main editor's text-selection state.
        if let Some(target) = self.text_field_drag {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.place_text_field_cursor(target, mouse.column, mouse.row, true);
                },
                MouseEventKind::Up(MouseButton::Left) => self.text_field_drag = None,
                _ => {},
            }
            return;
        }
        // A read-only surface's selection drag captures motion the same way the
        // editor's does, and for the same reason: the pointer must keep extending
        // even after it leaves the rows it started on.
        if self.surface_selecting.is_some() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.drag_surface_selection(mouse.column, mouse.row);
                },
                MouseEventKind::Up(MouseButton::Left) => self.surface_selecting = None,
                _ => {},
            }
            return;
        }
        // An in-progress text selection captures motion and the wheel until the
        // button is released (the release and any press were taken above).
        if self.editor_drag.is_some() {
            self.editor_drag_mouse(&mouse);
            return;
        }
        // A terminal selection drag captures motion the same way, until release.
        #[cfg(feature = "terminal")]
        if self.view == View::Editor
            && self.active_terminal().is_some_and(|pane| pane.selecting)
            && self.terminal_mouse(mouse)
        {
            return;
        }
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(divider) = self
                .last_frame
                .pane_dividers
                .iter()
                .copied()
                .find(|divider| divider.contains(mouse.column, mouse.row))
        {
            self.pane_resize = Some(PaneResize { divider });
            self.pane_divider_hover = Some(divider);
            return;
        }
        let pressed_in_console = self.settle_overlays_on_press(mouse);
        // Ahead of every region test: a track sits inside the rect of whatever it
        // scrolls, and those rects were narrowed to exclude it precisely so the bar
        // could claim the column.
        if self.background_scrollbar_mouse(mouse) || self.scrollbar_mouse(mouse) {
            return;
        }
        // The popups float over the editor, so they claim the pointer over them
        // before anything beneath them can.
        if self.handle_completion_mouse(mouse) || self.handle_hover_mouse(mouse) {
            return;
        }
        // The console floats over whatever it overlaps, so a press on it is the
        // console's and must not reach the editor underneath.
        if pressed_in_console {
            return;
        }
        if self.handle_tabstrip_mouse(mouse) {
            return;
        }
        if self.handle_breadcrumb_mouse(mouse) {
            return;
        }
        if self.handle_status_mouse(mouse) {
            return;
        }
        if self.handle_view_chrome_mouse(mouse) {
            return;
        }
        // Gated on the view that painted them: these read hit regions another view
        // draws over without re-rendering, so without it last frame's rects would keep
        // claiming clicks aimed at the view on screen.
        if self.view == View::GitHub && self.github_mouse(mouse) {
            return;
        }
        // Ahead of the region match below, and therefore ahead of the editor: the Seam
        // view owns its whole surface, and a press that fell through to `handle_editor_click`
        // would arm a text drag in a view that has no text to drag.
        if self.view == View::Editor && self.seam_mouse(mouse) {
            return;
        }
        // Likewise a terminal owns its grid: selection, the wheel, links, and the
        // reports a mouse-tracking program asked for.
        #[cfg(feature = "terminal")]
        if self.view == View::Editor && self.terminal_mouse(mouse) {
            return;
        }
        if self.handle_inlay_hint_mouse(&mouse) || self.handle_code_lens_mouse(&mouse) {
            return;
        }
        if self.handle_blame_mouse(mouse) {
            return;
        }
        if self.handle_markdown_link_mouse(mouse) {
            return;
        }
        let point = (mouse.column, mouse.row);
        let in_sidebar = self.sidebar_visible && rect_contains(self.last_frame.sidebar_rect, point);
        let in_outline = self.outline.visible && rect_contains(self.outline.rect, point);
        let in_editor = rect_contains(self.last_frame.editor_rect(), point);
        if in_editor && !in_outline && !matches!(mouse.kind, MouseEventKind::Moved) {
            self.dismiss_outline_overlay();
        }
        // The console floats over everything it overlaps, so its wheel is claimed
        // before any pane's — otherwise a notch over it would scroll the editor
        // behind it.
        let in_console = self.commit_console.open && rect_contains(self.commit_console.rect, point);
        // Past every surface that floats over the panes, the wheel scrolls the pane
        // under the pointer — focused or not — and leaves the focus alone.
        if !in_console && !in_outline && !in_sidebar && self.pane_wheel(mouse) {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown if in_console => self.commit_console_scroll(3),
            MouseEventKind::ScrollUp if in_console => self.commit_console_scroll(-3),
            MouseEventKind::ScrollDown if in_outline => self.outline_step(1),
            MouseEventKind::ScrollUp if in_outline => self.outline_step(-1),
            MouseEventKind::ScrollDown if in_sidebar => self.sidebar_wheel(3, mouse.row),
            MouseEventKind::ScrollUp if in_sidebar => self.sidebar_wheel(-3, mouse.row),
            MouseEventKind::ScrollDown => self.scroll_lines(3),
            MouseEventKind::ScrollUp => self.scroll_lines(-3),
            MouseEventKind::Down(MouseButton::Left) if in_outline => {
                self.handle_outline_click(mouse.row);
            },
            // Past the tab strip, which closes a tab on a middle click.
            MouseEventKind::Down(MouseButton::Middle) if !in_sidebar && !in_outline => {
                self.middle_click_paste(mouse);
            },
            MouseEventKind::Down(MouseButton::Right)
                if in_sidebar && self.sidebar_panel == SidebarPanel::Explorer =>
            {
                let row = rect_contains(self.last_frame.sidebar_content_rect, point)
                    .then(|| {
                        self.explorer.visible_index(
                            (mouse.row - self.last_frame.sidebar_content_rect.y) as usize,
                        )
                    })
                    .flatten();
                self.open_context_menu(mouse.column, mouse.row, row);
            },
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click in a pane's content area opens the pane context menu
                // for that pane's active tab.
                if let Some((pane, area)) = self
                    .last_frame
                    .panes
                    .iter()
                    .find(|f| rect_contains(f.content_rect, point))
                    .map(|f| (f.pane, f.editor_rect))
                {
                    self.focus_pane_switch(pane);
                    self.focus = Focus::Editor;
                    self.right_click_caret(area, mouse.column, mouse.row);
                    self.open_pane_context_menu(mouse.column, mouse.row, true);
                    self.add_spelling_to_menu(area, mouse.column, mouse.row);
                }
            },
            MouseEventKind::Down(MouseButton::Left) => {
                if self.sidebar_visible && mouse.column == self.last_frame.sidebar_divider_x {
                    // Grab the sidebar-width divider to start a resize drag.
                    self.sidebar_resizing = true;
                } else if in_sidebar
                    && self.sidebar_panel == SidebarPanel::SourceControl
                    && self.scm_ui.divider_y != 0
                    && mouse.row == self.scm_ui.divider_y
                {
                    // Grab the Source-Control changes/commits divider.
                    self.scm_ui.resizing = true;
                } else if in_sidebar {
                    self.handle_sidebar_click(mouse.column, mouse.row, mouse.modifiers);
                } else if !self.handle_find_bar_click(mouse) {
                    self.commit_input.focused = false;
                    // A read-only surface owns the click before the editor sees it;
                    // the editor's own handler only answers for code tabs.
                    if !self.begin_surface_selection(mouse.column, mouse.row) {
                        self.handle_editor_click(mouse);
                    }
                }
            },
            // Track the hover position for the secondary-accent row highlight in the
            // explorer / source-control lists (cleared when off the content area).
            MouseEventKind::Moved => {
                self.hover =
                    rect_contains(self.last_frame.sidebar_content_rect, point).then_some(point);
                self.pane_action_hover = None;
                self.update_language_server_hover(mouse.column, mouse.row);
                self.pane_divider_hover = self
                    .last_frame
                    .pane_dividers
                    .iter()
                    .copied()
                    .find(|divider| divider.contains(mouse.column, mouse.row));
                self.sidebar_header_hover =
                    (in_sidebar && mouse.row == self.last_frame.sidebar_rect.y).then_some(point);
                self.markdown_link_hover = self
                    .last_frame
                    .markdown_link_hits()
                    .any(|hit| rect_contains(hit.rect, point))
                    .then_some(point);
            },
            _ => {},
        }
    }

    /// The source-control change index under the hover cursor, if any (mirrors the
    /// SCM click hit-testing, using the last frame's row map).
    pub(crate) fn hovered_scm_change(&self) -> Option<usize> {
        let point = self.hover?;
        if !rect_contains(self.scm_ui.changes_rect, point) {
            return None;
        }
        let display = self.scm_ui.offset + usize::from(point.1 - self.scm_ui.changes_rect.y);
        self.scm_ui.row_map.get(display).copied().flatten()
    }

    /// Whether the pointer is over the Source-Control `COMMITS` title, which opens the
    /// full commit-graph view.
    pub(crate) fn hovered_scm_commits_title(&self) -> bool {
        self.hover
            .is_some_and(|point| rect_contains(self.scm_ui.commits_title_rect, point))
    }

    /// Handle a left click inside the sidebar: switch panels via the header (a
    /// double-click on the open panel's tab folds the sidebar), or select the
    /// clicked row. A plain click moves the cursor and activates the
    /// row; Ctrl toggles it in the selection and Shift extends a range to it
    /// (neither activates).
    pub(super) fn handle_sidebar_click(&mut self, col: u16, row_y: u16, modifiers: KeyModifiers) {
        if self.focus != Focus::Sidebar {
            // Arriving by click lands on the Problems list, not a filter edit
            // abandoned when focus left; a click on the filter field re-enters it.
            self.problems.filter_input = false;
        }
        self.focus = Focus::Sidebar;
        // The AI chip sits on the commit box's border, so it is tested before the
        // box itself — and only when it was actually painted, since the rect is
        // cleared each frame.
        if self.sidebar_panel == SidebarPanel::SourceControl
            && rect_contains(self.scm_ui.ai_chip_rect, (col, row_y))
        {
            self.commit_input.focused = true;
            if self.ai_commit.generating().is_some() {
                self.commit_generate_cancel();
            } else {
                self.commit_generate();
            }
            return;
        }
        if self.sidebar_panel == SidebarPanel::SourceControl
            && rect_contains(self.scm_ui.commit_rect, (col, row_y))
        {
            self.commit_input.focused = true;
            self.commit_input.place_cursor(
                col.saturating_sub(self.scm_ui.commit_rect.x),
                row_y.saturating_sub(self.scm_ui.commit_rect.y),
                self.scm_ui.commit_rect.width,
                modifiers.contains(KeyModifiers::SHIFT),
            );
            self.text_field_drag = Some(TextFieldTarget::Commit);
            return;
        }
        self.commit_input.focused = false;
        // Explorer header toolbar buttons sit on the header row alongside the switcher.
        if row_y == self.last_frame.sidebar_rect.y
            && let Some(cmd) = self
                .last_frame
                .header_action_hits
                .iter()
                .find_map(|&(start, end, cmd)| (col >= start && col < end).then_some(cmd))
        {
            self.dispatch(cmd);
            return;
        }
        if let Some(panel) = self.panel_at(col, row_y) {
            let streak = self.click_streak(col, row_y);
            if panel == self.sidebar_panel && streak >= 2 {
                // A double-click on the tab already open folds the sidebar (#315).
                self.dispatch(Command::ToggleSidebar);
            } else {
                if panel != self.sidebar_panel {
                    // A click that switches panels starts no streak, so a
                    // double-click on another tab opens it rather than opening it
                    // and folding it away again.
                    self.last_click = None;
                }
                self.dispatch(Command::SelectPanel(panel));
            }
            return;
        }
        if self.sidebar_panel == SidebarPanel::SourceControl
            && let Some(command) =
                self.scm_ui
                    .header_hits
                    .iter()
                    .find_map(|&(start, end, row, command)| {
                        (row_y == row && col >= start && col < end).then_some(command)
                    })
        {
            self.dispatch(command);
            return;
        }
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match self.sidebar_panel {
            SidebarPanel::Spelling => self.spelling_click(col, row_y),
            SidebarPanel::Todos => self.todos_click(col, row_y),
            SidebarPanel::Debug => self.debug_click(col, row_y),
            SidebarPanel::Problems => self.problems_click(col, row_y),
            SidebarPanel::Hierarchy => self.hierarchy_click(col, row_y),
            SidebarPanel::Explorer => {
                if !rect_contains(self.last_frame.sidebar_content_rect, (col, row_y)) {
                    return;
                }
                // An open inline rename owns clicks on its own row: they place the
                // caret in the name rather than selecting a different entry.
                if self
                    .explorer
                    .edit_rect()
                    .is_some_and(|rect| rect_contains(rect, (col, row_y)))
                {
                    self.place_text_field_cursor(
                        TextFieldTarget::ExplorerRename,
                        col,
                        row_y,
                        shift,
                    );
                    self.text_field_drag = Some(TextFieldTarget::ExplorerRename);
                    return;
                }
                let view_row = (row_y - self.last_frame.sidebar_content_rect.y) as usize;
                let root = self.root.clone();
                self.explorer.ensure_built(&root);
                if self.explorer.visible_index(view_row).is_none() {
                    return;
                }
                if ctrl {
                    self.explorer.toggle_visible(view_row);
                } else if shift {
                    self.explorer.extend_visible(view_row);
                } else {
                    let streak = self.click_streak(col, row_y);
                    self.explorer.select_visible(view_row);
                    if streak >= 2 {
                        // Double-click lands on the SAME view the single-click
                        // preview created, materialized — never a duplicate.
                        self.sidebar_promote_or_open_permanent();
                    } else {
                        self.explorer_preview_with_focus();
                    }
                }
            },
            SidebarPanel::SourceControl => {
                // The pinned commit-log region: click a commit row to open its commit
                // view; the trailing "load more" affordance pages in older history.
                if rect_contains(self.scm_ui.commits_title_rect, (col, row_y)) {
                    self.dispatch(Command::ShowCommitGraph);
                    return;
                }
                if rect_contains(self.scm_ui.commits_rect, (col, row_y)) {
                    let display =
                        self.scm_ui.commits_offset + (row_y - self.scm_ui.commits_rect.y) as usize;
                    if self.scm_ui.more_row == Some(display) {
                        self.load_more_scm_log();
                    } else if let Some(commit) = self.scm.log.get(display) {
                        self.open_commit(commit.hash.clone());
                    }
                    return;
                }
                if !rect_contains(self.scm_ui.changes_rect, (col, row_y)) {
                    return;
                }
                let display = self.scm_ui.offset + (row_y - self.scm_ui.changes_rect.y) as usize;
                if let Some(Some(idx)) = self.scm_ui.row_map.get(display).copied() {
                    if ctrl {
                        self.scm.selection.toggle(idx);
                    } else if shift {
                        self.scm.selection.extend_to(idx);
                    } else {
                        let streak = self.click_streak(col, row_y);
                        self.scm.selection.move_to(idx);
                        if streak >= 2 {
                            // Double-click materializes the SAME view the
                            // single-click preview created and moves focus into
                            // it — never a duplicate diff tab.
                            self.open_selected_diff();
                        } else {
                            // A single click browses: the diff shows in the
                            // preview slot while the panel keeps focus, so the
                            // staging keys stay live.
                            self.preview_selected_diff();
                        }
                    }
                }
            },
            SidebarPanel::Search => {
                // Header buttons: option toggles on the find row, replace-all on the
                // replace row.
                if let Some(cmd) =
                    self.search_ui
                        .action_hits
                        .iter()
                        .find_map(|&(start, end, ry, cmd)| {
                            (row_y == ry && col >= start && col < end).then_some(cmd)
                        })
                {
                    self.dispatch(cmd);
                    return;
                }
                // Click a field to edit it.
                if rect_contains(self.search_ui.query_rect, (col, row_y)) {
                    self.search.field = SearchPanelField::Find;
                    self.search.input = true;
                    self.place_text_field_cursor(TextFieldTarget::SearchFind, col, row_y, shift);
                    self.text_field_drag = Some(TextFieldTarget::SearchFind);
                    return;
                }
                if self
                    .search_ui
                    .replace_rect
                    .is_some_and(|rect| rect_contains(rect, (col, row_y)))
                {
                    self.search.field = SearchPanelField::Replace;
                    self.search.replace_visible = true;
                    self.search.input = true;
                    self.place_text_field_cursor(TextFieldTarget::SearchReplace, col, row_y, shift);
                    self.text_field_drag = Some(TextFieldTarget::SearchReplace);
                    return;
                }
                for (rect, field, target) in [
                    (
                        self.search_ui.includes_rect,
                        SearchPanelField::Includes,
                        TextFieldTarget::SearchIncludes,
                    ),
                    (
                        self.search_ui.excludes_rect,
                        SearchPanelField::Excludes,
                        TextFieldTarget::SearchExcludes,
                    ),
                ] {
                    if rect.is_some_and(|rect| rect_contains(rect, (col, row_y))) {
                        self.search.field = field;
                        self.search.input = true;
                        self.place_text_field_cursor(target, col, row_y, shift);
                        self.text_field_drag = Some(target);
                        return;
                    }
                }
                if !rect_contains(self.search_ui.results_rect, (col, row_y)) {
                    return;
                }
                let idx = self.search_ui.offset + (row_y - self.search_ui.results_rect.y) as usize;
                if idx >= self.search.rows.len() {
                    return;
                }
                // Taken once, before the branch: the streak counter is shared with
                // every other click surface, so reading it twice would advance it.
                let streak = self.click_streak(col, row_y);
                self.search.selection.move_to(idx);
                // The click moves the panel's focus onto the results, so the arrow
                // keys that follow navigate the list rather than the last field.
                self.search.input = false;
                // A click on a file heading's chevron toggles the group; anywhere
                // else on the row opens the file, so the affordance is the glyph.
                // The chevron renders as glyph-plus-space, so both cells toggle;
                // a one-cell target is too fine to hit reliably.
                let on_chevron = col <= self.search_ui.results_rect.x.saturating_add(1);
                // Double-clicking a heading is the second fold affordance: the first
                // click has already opened the file, the second folds the group. A
                // match row is a leaf, so it just opens again.
                match self.search.rows.get(idx) {
                    Some(SearchRow::File { .. }) if on_chevron || streak >= 2 => {
                        self.search_toggle_row();
                    },
                    _ => self.open_selected_result(),
                }
            },
        }
    }
}
