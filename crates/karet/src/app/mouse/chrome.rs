//! Mouse handling for the window chrome — tab strips, breadcrumbs, toasts,
//! the status bar, blame, markdown links, and context menus — split from the
//! core mouse module to respect the per-file code-line ceiling.

use super::*;

impl App {
    /// Handle a mouse event over a pane's tab strip (click to switch / close, wheel
    /// to cycle). Returns `true` when the event was consumed.
    pub(in crate::app) fn handle_tabstrip_mouse(&mut self, mouse: MouseEvent) -> bool {
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
                        self.tab_drag = Some(TabDrag {
                            from_pane: pane,
                            hover: None,
                        });
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
    pub(in crate::app) fn handle_breadcrumb_mouse(&mut self, mouse: MouseEvent) -> bool {
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
    pub(in crate::app) fn handle_toast_mouse(&mut self, mouse: MouseEvent) -> bool {
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
    pub(in crate::app) fn status_command_at(&self, x: u16) -> Option<Command> {
        self.status_hits
            .iter()
            .find_map(|&(start, end, cmd)| (x >= start && x < end).then_some(cmd))
    }

    /// Handle a left click on a status-bar segment. Returns `true` when consumed.
    pub(in crate::app) fn handle_status_mouse(&mut self, mouse: MouseEvent) -> bool {
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

    /// Handle a left click on the top-level view switcher. Returns `true` when the
    /// event was over the chrome row — clicks there never fall through to the body.
    pub(in crate::app) fn handle_view_chrome_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !rect_contains(self.view_chrome_rect, (mouse.column, mouse.row)) {
            return false;
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind
            && let Some(view) = self.view_hits.iter().find_map(|&(start, end, view)| {
                (mouse.column >= start && mouse.column < end).then_some(view)
            })
        {
            self.dispatch(Command::SelectView(view));
        }
        true
    }

    /// Open the attributed commit when the visible inline blame label is
    /// double-clicked. The first click is consumed so it cannot fall through to the
    /// editor and count twice in the shared multi-click streak.
    pub(in crate::app) fn handle_blame_mouse(&mut self, mouse: MouseEvent) -> bool {
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
    pub(in crate::app) fn handle_markdown_link_mouse(&mut self, mouse: MouseEvent) -> bool {
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

    pub(in crate::app) fn open_markdown_file_link(&mut self, path: &Path) {
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
    pub(in crate::app) fn handle_context_menu_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(menu) = self.context_menu.as_ref() else {
            return false;
        };
        let point = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if rect_contains(menu.rect, point) => {
                // The click and the hover accent resolve rows the same way, so
                // the row that lit up is the row that runs.
                let row = menu.row_at(mouse.column, mouse.row);
                if let (Some(menu), Some(row)) = (self.context_menu.as_mut(), row) {
                    menu.selected = row;
                }
                self.accept_context_menu();
                true
            },
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right) => {
                self.close_context_menu();
                true
            },
            // An open menu swallows every other event; spend the pointer motion
            // on live feedback rather than dropping it.
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if let Some(menu) = self.context_menu.as_mut() {
                    menu.set_hover(Some(point));
                }
                true
            },
            _ => true,
        }
    }
}
