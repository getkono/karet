use super::*;

impl App {
    /// Handle a mouse event over a pane's tab strip (click to switch / close, wheel
    /// to cycle). Returns `true` when the event was consumed.
    pub(super) fn handle_tabstrip_mouse(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        let Some((pane, hit, action)) = self.pane_frames.iter().find_map(|f| {
            rect_contains(f.tabstrip_rect, point).then(|| {
                let action = f.action_hits.iter().find_map(|&(start, end, command)| {
                    (mouse.column >= start && mouse.column < end).then_some(command)
                });
                (f.pane, tab_at(&f.tab_hits, mouse.column), action)
            })
        }) else {
            return false;
        };
        // Act on the clicked pane (borrow of `pane_frames` has ended).
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.focus_pane_switch(pane);
                self.next_tab();
            },
            MouseEventKind::ScrollUp => {
                self.focus_pane_switch(pane);
                self.prev_tab();
            },
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus_pane_switch(pane);
                if let Some(command) = action {
                    self.dispatch(command);
                } else if let Some((i, on_close)) = hit {
                    if on_close {
                        self.request_close_tab_at(i);
                    } else {
                        self.select_tab(i);
                        if !self.tabs[i].is_github_dashboard() {
                            self.tab_drag = Some(TabDrag {
                                from_pane: pane,
                                hover: None,
                            });
                        }
                    }
                }
            },
            MouseEventKind::Down(MouseButton::Middle) => {
                self.focus_pane_switch(pane);
                if action.is_none()
                    && let Some((i, _)) = hit
                {
                    self.request_close_tab_at(i);
                }
            },
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click on a tab selects it and opens the pane context menu
                // for it; the strip's empty tail opens nothing.
                self.focus_pane_switch(pane);
                if action.is_none()
                    && let Some((i, _)) = hit
                {
                    self.select_tab(i);
                    self.open_pane_context_menu(mouse.column, mouse.row);
                }
            },
            MouseEventKind::Moved => {
                self.pane_action_hover = action.map(|_| point);
            },
            _ => {},
        }
        true
    }

    /// Handle a left click on a pane's breadcrumb row: a segment reveals its path
    /// prefix in the Explorer; a separator gap (or an inert segment above the
    /// workspace root) does nothing. Either way the click is consumed so it never
    /// falls through to the tab strip or editor underneath. Returns `true` when
    /// consumed.
    pub(super) fn handle_breadcrumb_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        let point = (mouse.column, mouse.row);
        let Some(hit) = self.pane_frames.iter().find_map(|f| {
            rect_contains(f.breadcrumb_rect, point).then(|| {
                f.breadcrumb_hits
                    .iter()
                    .find(|h| mouse.column >= h.start && mouse.column < h.end)
                    .map(|h| h.path.clone())
            })
        }) else {
            return false;
        };
        if let Some(path) = hit {
            self.reveal_in_explorer(&path);
        }
        true
    }

    /// Handle a click on a toast card: left-click dismisses it, while right-click
    /// copies an error's complete text. Returns `true` when the click landed on a
    /// card (so it is not routed elsewhere).
    pub(super) fn handle_toast_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
        ) {
            return false;
        }
        let point = (mouse.column, mouse.row);
        let Some(hit) = self
            .toast_hits
            .iter()
            .find(|h| rect_contains(h.rect, point))
        else {
            return false;
        };
        let id = hit.id;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.notifications.dismiss(id),
            MouseEventKind::Down(MouseButton::Right) => {
                let text = self
                    .notifications
                    .active()
                    .into_iter()
                    .find(|note| note.id == id && note.severity == Severity::Error)
                    .map(notification_clipboard_text);
                if let Some(text) = text {
                    self.copy_to_clipboard(text, "error");
                }
            },
            _ => {},
        }
        true
    }

    /// The command bound to the status-bar segment at column `x`, if any.
    pub(super) fn status_command_at(&self, x: u16) -> Option<Command> {
        self.status_hits
            .iter()
            .find_map(|&(start, end, cmd)| (x >= start && x < end).then_some(cmd))
    }

    /// Handle a left click on a status-bar segment. Returns `true` when consumed.
    pub(super) fn handle_status_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !rect_contains(self.status_rect, (mouse.column, mouse.row)) {
            return false;
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind
            && let Some(cmd) = self.status_command_at(mouse.column)
        {
            self.dispatch(cmd);
        }
        true
    }

    /// Open the attributed commit when the visible inline blame label is
    /// double-clicked. The first click is consumed so it cannot fall through to the
    /// editor and count twice in the shared multi-click streak.
    pub(super) fn handle_blame_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            || !self
                .blame_rect
                .is_some_and(|rect| rect_contains(rect, (mouse.column, mouse.row)))
        {
            return false;
        }
        if self.click_streak(mouse.column, mouse.row) >= 2 {
            self.open_live_blame_detail();
        }
        true
    }

    /// Activate a Markdown link only for the explicit Ctrl/Cmd-click gesture.
    pub(super) fn handle_markdown_link_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            || !mouse
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return false;
        }
        let point = (mouse.column, mouse.row);
        let Some(target) = self
            .markdown_link_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, point))
            .map(|hit| hit.target.clone())
        else {
            return false;
        };
        let Some(source) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            return false;
        };

        match crate::links::resolve(&target, &source, &self.root) {
            Ok(crate::links::LinkTarget::ExternalUrl(url)) => {
                if let Err(error) = crate::links::open_external(&url) {
                    self.notify(
                        Severity::Error,
                        NotificationKind::System,
                        format!("could not open link: {error}"),
                    );
                }
            },
            Ok(crate::links::LinkTarget::WorkspaceFile { path, .. }) => {
                self.open_markdown_file_link(&path);
            },
            Ok(crate::links::LinkTarget::OutsideWorkspaceFile(path)) => {
                self.overlay = Some(Overlay::text(
                    "Type open to open a file outside this workspace",
                    TextPurpose::ConfirmOutsideWorkspaceLink { path },
                ));
            },
            Err(error) => self.notify(
                Severity::Warning,
                NotificationKind::System,
                format!("link blocked: {error}"),
            ),
        }
        true
    }

    pub(super) fn open_markdown_file_link(&mut self, path: &Path) {
        if path.is_file() {
            self.open_path(path);
        } else {
            self.notify(
                Severity::Warning,
                NotificationKind::Io,
                format!("linked file does not exist: {}", path.display()),
            );
        }
    }

    /// Handle mouse interaction with an open context menu.
    pub(super) fn handle_context_menu_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(menu) = self.context_menu.as_ref() else {
            return false;
        };
        let point = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if rect_contains(menu.rect, point) => {
                let inner_y = mouse.row.saturating_sub(menu.rect.y).saturating_sub(1);
                let idx = usize::from(inner_y);
                if let Some(menu) = self.context_menu.as_mut()
                    && idx < menu.entries.len()
                {
                    menu.selected = idx;
                }
                self.accept_context_menu();
                true
            },
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right) => {
                self.close_context_menu();
                true
            },
            _ => true,
        }
    }

    /// Handle a mouse event: the tab strip (switch / close / cycle), wheel scrolls
    /// (the sidebar or the active tab), and a left click moves focus.
    /// Resize the sidebar so its right edge sits at column `col`. Dragging narrower
    /// than [`SIDEBAR_MIN_WIDTH`] is read as intent to collapse. The responsive upper
    /// bound (terminal width) is applied when the layout is next computed.
    pub(super) fn resize_sidebar_to(&mut self, col: u16) {
        let width = col.saturating_sub(self.sidebar_rect.x);
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
        let content = self.sidebar_content_rect;
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
        let over_sidebar_divider = self.sidebar_resizing
            || (self.sidebar_visible && mouse.column == self.sidebar_divider_x);
        let over_scm_divider = self.scm_ui.resizing
            || (self.sidebar_panel == SidebarPanel::SourceControl
                && self.scm_ui.divider_y != 0
                && mouse.row == self.scm_ui.divider_y
                && rect_contains(self.sidebar_rect, (mouse.column, mouse.row)));
        let pane_axis = self
            .pane_resize
            .map(|resize| resize.divider.axis)
            .or_else(|| {
                self.pane_dividers
                    .iter()
                    .find(|divider| divider.contains(mouse.column, mouse.row))
                    .map(|divider| divider.axis)
            });
        let over_blame = self
            .blame_rect
            .is_some_and(|rect| rect_contains(rect, (mouse.column, mouse.row)));
        let over_markdown_link = self
            .markdown_link_hits
            .iter()
            .any(|hit| rect_contains(hit.rect, (mouse.column, mouse.row)));
        let shape = if over_sidebar_divider || pane_axis == Some(SplitAxis::Cols) {
            Some("col-resize")
        } else if over_scm_divider || pane_axis == Some(SplitAxis::Rows) {
            Some("row-resize")
        } else if over_blame || over_markdown_link {
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
        self.request_live_blame();
    }

    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        self.update_pointer_shape_hint(&mouse);
        // Toasts float above everything (including the overlay), so hit-test them
        // first: left-click dismisses and right-click copies an error.
        if self.handle_toast_mouse(mouse) {
            return;
        }
        if self.overlay.is_some() {
            return;
        }
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
                        .dividers(self.main_rect)
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
                        self.layout
                            .resize_divider(resize.divider, delta, self.main_rect);
                    }
                },
                MouseEventKind::Up(MouseButton::Left) => self.pane_resize = None,
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
        // An in-progress text selection captures motion until the button is released.
        if self.editor_selecting {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.drag_select_to(mouse.column, mouse.row);
                },
                MouseEventKind::Up(MouseButton::Left) => self.editor_selecting = false,
                _ => {},
            }
            return;
        }
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(divider) = self
                .pane_dividers
                .iter()
                .copied()
                .find(|divider| divider.contains(mouse.column, mouse.row))
        {
            self.pane_resize = Some(PaneResize { divider });
            self.pane_divider_hover = Some(divider);
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
        if self.github_mouse(mouse) {
            return;
        }
        if self.handle_blame_mouse(mouse) {
            return;
        }
        if self.handle_markdown_link_mouse(mouse) {
            return;
        }
        let point = (mouse.column, mouse.row);
        let in_sidebar = self.sidebar_visible && rect_contains(self.sidebar_rect, point);
        let in_outline = self.outline.visible && rect_contains(self.outline.rect, point);
        let in_editor = rect_contains(self.editor_rect, point);
        let in_markdown_preview = rect_contains(self.markdown_preview_rect, point);
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        if in_editor && !in_outline && !matches!(mouse.kind, MouseEventKind::Moved) {
            self.dismiss_outline_overlay();
        }
        match mouse.kind {
            MouseEventKind::ScrollDown if in_outline => self.outline_step(1),
            MouseEventKind::ScrollUp if in_outline => self.outline_step(-1),
            MouseEventKind::ScrollDown if in_sidebar => self.sidebar_wheel(3, mouse.row),
            MouseEventKind::ScrollUp if in_sidebar => self.sidebar_wheel(-3, mouse.row),
            MouseEventKind::ScrollDown if in_markdown_preview => {
                self.scroll_markdown_preview(3);
            },
            MouseEventKind::ScrollUp if in_markdown_preview => {
                self.scroll_markdown_preview(-3);
            },
            MouseEventKind::ScrollRight if in_editor => self.scroll_columns(3),
            MouseEventKind::ScrollLeft if in_editor => self.scroll_columns(-3),
            MouseEventKind::ScrollDown if in_editor && shift => self.scroll_columns(3),
            MouseEventKind::ScrollUp if in_editor && shift => self.scroll_columns(-3),
            MouseEventKind::ScrollDown => self.scroll_lines(3),
            MouseEventKind::ScrollUp => self.scroll_lines(-3),
            MouseEventKind::Down(MouseButton::Left) if in_outline => {
                self.handle_outline_click(mouse.row);
            },
            MouseEventKind::Down(MouseButton::Right)
                if in_sidebar && self.sidebar_panel == SidebarPanel::Explorer =>
            {
                let row = rect_contains(self.sidebar_content_rect, point)
                    .then(|| {
                        self.explorer
                            .visible_index((mouse.row - self.sidebar_content_rect.y) as usize)
                    })
                    .flatten();
                self.open_context_menu(mouse.column, mouse.row, row);
            },
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click in a pane's content area opens the pane context menu
                // for that pane's active tab.
                if let Some(pane) = self
                    .pane_frames
                    .iter()
                    .find(|f| rect_contains(f.content_rect, point))
                    .map(|f| f.pane)
                {
                    self.focus_pane_switch(pane);
                    self.focus = Focus::Editor;
                    self.open_pane_context_menu(mouse.column, mouse.row);
                }
            },
            MouseEventKind::Down(MouseButton::Left) => {
                if self.sidebar_visible && mouse.column == self.sidebar_divider_x {
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
                } else {
                    self.commit_input.focused = false;
                    self.handle_editor_click(mouse);
                }
            },
            // Track the hover position for the secondary-accent row highlight in the
            // explorer / source-control lists (cleared when off the content area).
            MouseEventKind::Moved => {
                self.hover = rect_contains(self.sidebar_content_rect, point).then_some(point);
                self.pane_action_hover = None;
                self.update_language_server_hover(mouse.column, mouse.row);
                self.pane_divider_hover = self
                    .pane_dividers
                    .iter()
                    .copied()
                    .find(|divider| divider.contains(mouse.column, mouse.row));
                self.sidebar_header_hover =
                    (in_sidebar && mouse.row == self.sidebar_rect.y).then_some(point);
                self.markdown_link_hover = self
                    .markdown_link_hits
                    .iter()
                    .any(|hit| rect_contains(hit.rect, point))
                    .then_some(point);
            },
            _ => {},
        }
    }

    /// The absolute explorer row index under the hover cursor, if the mouse is over
    /// the explorer's content (accounts for the current scroll offset).
    pub(crate) fn hovered_explorer_row(&self) -> Option<usize> {
        let (_, hy) = self.hover?;
        let top = self.sidebar_content_rect.y;
        (hy >= top).then(|| self.explorer.offset() + usize::from(hy - top))
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

    /// The sidebar panel whose header switcher cell is at `(col, row_y)`, if any.
    pub(super) fn panel_at(&self, col: u16, row_y: u16) -> Option<SidebarPanel> {
        if row_y != self.sidebar_rect.y {
            return None;
        }
        self.panel_hits
            .iter()
            .find_map(|&(start, end, panel)| (col >= start && col < end).then_some(panel))
    }

    /// Handle a left click inside the sidebar: switch panels via the header, or
    /// select the clicked row. A plain click moves the cursor and activates the
    /// row; Ctrl toggles it in the selection and Shift extends a range to it
    /// (neither activates).
    pub(super) fn handle_sidebar_click(&mut self, col: u16, row_y: u16, modifiers: KeyModifiers) {
        self.focus = Focus::Sidebar;
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
        if row_y == self.sidebar_rect.y
            && let Some(cmd) = self
                .header_action_hits
                .iter()
                .find_map(|&(start, end, cmd)| (col >= start && col < end).then_some(cmd))
        {
            self.dispatch(cmd);
            return;
        }
        if let Some(panel) = self.panel_at(col, row_y) {
            self.dispatch(Command::SelectPanel(panel));
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
            SidebarPanel::Explorer => {
                if !rect_contains(self.sidebar_content_rect, (col, row_y)) {
                    return;
                }
                let view_row = (row_y - self.sidebar_content_rect.y) as usize;
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
                if rect_contains(self.scm_ui.commits_rect, (col, row_y)) {
                    let display =
                        self.scm_ui.commits_offset + (row_y - self.scm_ui.commits_rect.y) as usize;
                    if self.scm_ui.more_row == Some(display) {
                        self.load_more_scm_log();
                    } else if let Some(commit) =
                        display.checked_sub(1).and_then(|i| self.scm.log.get(i))
                    {
                        // Row 0 is the " COMMITS" header; commits begin at display 1.
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
                    self.search.field = SearchField::Find;
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
                    self.search.field = SearchField::Replace;
                    self.search.replace_visible = true;
                    self.search.input = true;
                    self.place_text_field_cursor(TextFieldTarget::SearchReplace, col, row_y, shift);
                    self.text_field_drag = Some(TextFieldTarget::SearchReplace);
                    return;
                }
                if !rect_contains(self.search_ui.results_rect, (col, row_y)) {
                    return;
                }
                let idx = self.search_ui.offset + (row_y - self.search_ui.results_rect.y) as usize;
                if idx < self.search.results.len() {
                    self.search.selected = idx;
                    self.open_selected_result();
                }
            },
        }
    }

    fn place_text_field_cursor(
        &mut self,
        target: TextFieldTarget,
        column: u16,
        row: u16,
        extend: bool,
    ) {
        match target {
            TextFieldTarget::Commit => self.commit_input.place_cursor(
                column.saturating_sub(self.scm_ui.commit_rect.x),
                row.saturating_sub(self.scm_ui.commit_rect.y),
                self.scm_ui.commit_rect.width,
                extend,
            ),
            TextFieldTarget::SearchFind => {
                let cell = usize::from(
                    column
                        .saturating_sub(self.search_ui.query_rect.x)
                        .saturating_add(self.search.query_edit.scroll),
                );
                let cursor = karet_widgets::textfield::byte_at_cell(&self.search.query, cell);
                self.search
                    .query_edit
                    .set_cursor(&self.search.query, cursor, extend);
            },
            TextFieldTarget::SearchReplace => {
                let Some(rect) = self.search_ui.replace_rect else {
                    return;
                };
                let cell = usize::from(
                    column
                        .saturating_sub(rect.x)
                        .saturating_add(self.search.replace_edit.scroll),
                );
                let cursor = karet_widgets::textfield::byte_at_cell(&self.search.replace, cell);
                self.search
                    .replace_edit
                    .set_cursor(&self.search.replace, cursor, extend);
            },
        }
    }
}

pub(super) fn notification_clipboard_text(notification: &Notification) -> String {
    notification.body.as_ref().map_or_else(
        || notification.title.clone(),
        |body| format!("{}\n{body}", notification.title),
    )
}
