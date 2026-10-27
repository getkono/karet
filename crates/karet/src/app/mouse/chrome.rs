//! Mouse handling for the window chrome — tab strips, breadcrumbs, toasts,
//! the status bar, blame, markdown links, and context menus — and for routing
//! the pointer to what is under it: background panes, and the completion,
//! hover and diagnostic popups. Split from the core mouse module to respect the
//! per-file code-line ceiling.

use super::*;

impl App {
    /// Handle a mouse event over a pane's tab strip (click to switch / close, wheel
    /// to cycle). Returns `true` when the event was consumed.
    pub(in crate::app) fn handle_tabstrip_mouse(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        let Some((pane, hit, action)) = self.last_frame.panes.iter().find_map(|f| {
            rect_contains(f.tabstrip_rect, point).then(|| {
                let action = f.action_hits.iter().find_map(|&(start, end, command)| {
                    (mouse.column >= start && mouse.column < end).then_some(command)
                });
                (f.pane, tab_at(&f.tab_hits, mouse.column), action)
            })
        }) else {
            return false;
        };
        // Act on the clicked pane (borrow of `last_frame.panes` has ended).
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
                    self.open_pane_context_menu(mouse.column, mouse.row, false);
                }
            },
            MouseEventKind::Moved => {
                self.pane_action_hover = action.map(|_| point);
            },
            _ => {},
        }
        true
    }

    /// Handle a left click on a pane's breadcrumb row: a path segment reveals its
    /// prefix in the Explorer, and a symbol segment focuses its pane and opens
    /// the picker over that symbol's siblings; a separator gap (or an inert
    /// segment above the workspace root) does nothing. Either way the click is
    /// consumed so it never falls through to the tab strip or editor underneath.
    /// Returns `true` when consumed.
    pub(in crate::app) fn handle_breadcrumb_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        let point = (mouse.column, mouse.row);
        // The badge is checked before the path segments because it is painted over
        // the row's right edge, which the breadcrumb no longer occupies.
        if self.last_frame.panes.iter().any(|f| {
            rect_contains(f.breadcrumb_rect, point)
                && f.lsp_badge_hit
                    .is_some_and(|(start, end)| mouse.column >= start && mouse.column < end)
        }) {
            // Opening the manager is the whole of it: the badge reports a condition
            // and offers a place to act on it, but never acts by itself. An install
            // or a restart spends the user's bandwidth or kills a running process,
            // and neither is something a single click on a status glyph should do.
            self.dispatch(Command::ManageLanguageServers);
            return true;
        }
        let Some((pane, hit)) = self.last_frame.panes.iter().find_map(|f| {
            rect_contains(f.breadcrumb_rect, point).then(|| {
                let target = f
                    .breadcrumb_hits
                    .iter()
                    .find(|h| mouse.column >= h.start && mouse.column < h.end)
                    .map(|h| h.target.clone());
                (f.pane, target)
            })
        }) else {
            return false;
        };
        match hit {
            Some(BreadcrumbTarget::Path(path)) => self.reveal_in_explorer(&path),
            Some(BreadcrumbTarget::Symbol(depth)) => {
                // The chain names the pane's active tab, so the picker opens on it.
                self.focus_pane_switch(pane);
                self.open_sibling_symbols(depth);
            },
            None => {},
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
            .last_frame
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
        self.last_frame
            .status_hits
            .iter()
            .find_map(|&(start, end, cmd)| (x >= start && x < end).then_some(cmd))
    }

    /// Handle a left click on a status-bar segment. Returns `true` when consumed.
    pub(in crate::app) fn handle_status_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !rect_contains(self.last_frame.status_rect, (mouse.column, mouse.row)) {
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
        if !rect_contains(self.last_frame.view_chrome_rect, (mouse.column, mouse.row)) {
            return false;
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind
            && let Some(view) = self
                .last_frame
                .view_hits
                .iter()
                .find_map(|&(start, end, view)| {
                    (mouse.column >= start && mouse.column < end).then_some(view)
                })
        {
            self.dispatch(Command::SelectView(view));
        }
        true
    }

    /// Open the attributed commit when the visible inline blame label is
    /// double-clicked. The first click is consumed so it cannot fall through to the
    /// editor and count twice in the shared multi-click streak, but it still
    /// focuses the editor pane the label is in, as any click on its text does.
    pub(in crate::app) fn handle_blame_mouse(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            || !self
                .last_frame
                .blame_rect()
                .is_some_and(|rect| rect_contains(rect, point))
        {
            return false;
        }
        if let Some(pane) = self
            .last_frame
            .panes
            .iter()
            .find(|frame| rect_contains(frame.content_rect, point))
            .map(|frame| frame.pane)
        {
            self.focus_pane_switch(pane);
            self.focus = Focus::Editor;
        }
        if self.click_streak(mouse.column, mouse.row) >= 2 {
            self.open_live_blame_detail();
        }
        true
    }

    /// Activate a link only for the explicit Ctrl/Cmd-click gesture: one
    /// rendered in a Markdown preview, or one in the editor under the
    /// pointer, focused split or not -- a language server's document link
    /// first, else one written in the source (a bare URL, or a Markdown
    /// `[text](target)`).
    ///
    /// A link outranks Go to Definition under the same gesture: the link is
    /// what the underline promised. Alt or Shift keep the click the
    /// editor's (multi-caret, extend), as Ctrl+click on a symbol does.
    pub(in crate::app) fn handle_markdown_link_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            || !mouse
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return false;
        }
        let point = (mouse.column, mouse.row);
        let preview = self
            .last_frame
            .markdown_link_hits()
            .find(|hit| rect_contains(hit.rect, point))
            .map(|hit| hit.target.clone());
        let plain = !mouse
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        // A language server's link outranks one read from the source: the
        // server knows where the text leads, where a scan only recognises it.
        let server = if preview.is_none() && plain {
            self.pane_server_link(point)
        } else {
            None
        };
        let target = match (&server, preview) {
            (Some(_), _) => None,
            (None, Some(preview)) => Some(preview),
            (None, None) => self.pane_source_link(point).filter(|_| plain),
        };
        if server.is_none() && target.is_none() {
            return false;
        }
        // Every pane's links are live. The link resolves against the file it is
        // in, so the click focuses that pane first, as any click on its text does.
        if let Some(pane) = self
            .last_frame
            .panes
            .iter()
            .find(|frame| rect_contains(frame.content_rect, point))
            .map(|frame| frame.pane)
        {
            self.focus_pane_switch(pane);
            self.focus = Focus::Editor;
        }
        if let Some((doc, link)) = server {
            self.follow_document_link(doc, link);
            return true;
        }
        let Some(target) = target else {
            return false;
        };
        self.activate_link(&target)
    }

    /// Open the link `target`, as written in the active tab's file, the way a
    /// Ctrl/Cmd+click does: a URL with the desktop's opener, a workspace file
    /// in karet, a file outside the workspace only after a confirmation, and
    /// anything else refused with a notice. `false` when the active tab has no
    /// file for a relative target to resolve against.
    pub(in crate::app) fn activate_link(&mut self, target: &str) -> bool {
        let Some(source) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            return false;
        };

        match crate::links::resolve(target, &source, &self.root) {
            Ok(crate::links::LinkTarget::ExternalUrl(url)) => {
                if let Err(error) = crate::links::open_external(&url) {
                    self.notify(
                        Report::Failure,
                        NotificationKind::System,
                        format!("could not open link: {error}"),
                    );
                }
            },
            Ok(crate::links::LinkTarget::WorkspaceFile { path, .. }) => {
                self.open_markdown_file_link(&path);
            },
            Ok(crate::links::LinkTarget::OutsideWorkspaceFile(path)) => {
                self.confirm_outside_workspace_link(path);
            },
            Err(error) => self.notify(
                Report::Refusal,
                NotificationKind::System,
                format!("link blocked: {error}"),
            ),
        }
        true
    }

    /// Ask before opening `path`, a link's file outside the workspace.
    pub(in crate::app) fn confirm_outside_workspace_link(&mut self, path: PathBuf) {
        self.confirm_action(
            "Open a file outside this workspace?",
            format!(
                "This link points to {}, which is outside {}. Opening it \
                 leaves the workspace you started karet in.",
                path.display(),
                self.root.display()
            ),
            "Stay in the workspace",
            "Open it",
            ConfirmAction::OpenOutsideWorkspaceLink(path),
        );
    }

    pub(in crate::app) fn open_markdown_file_link(&mut self, path: &Path) {
        if path.is_file() {
            self.open_path(path);
        } else {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                format!("linked file does not exist: {}", path.display()),
            );
        }
    }

    /// Run `f` with `pane` standing in as the focused pane, then hand focus back.
    ///
    /// Every scroll path writes "the active tab", and only the focused pane's tabs
    /// are live in `tabs`/`active`. A wheel notch or a scrollbar gesture over a
    /// background pane borrows the focus for exactly that one write, so it lands
    /// on the pane under the pointer without moving the user's focus there. The
    /// swap is done by hand rather than through `focus_pane_switch`, which would
    /// record each borrowed tab as an activation in the view history.
    pub(in crate::app) fn with_pane(&mut self, pane: PaneId, f: impl FnOnce(&mut Self)) {
        let home = self.layout.focus();
        if pane == home {
            f(self);
            return;
        }
        let Some(visiting) = self.stored.remove(&pane) else {
            return;
        };
        let home_tabs = std::mem::replace(&mut self.tabs, visiting.tabs);
        let home_active = std::mem::replace(&mut self.active, visiting.active);
        self.layout.set_focus(pane);
        f(self);
        self.layout.set_focus(home);
        let tabs = std::mem::replace(&mut self.tabs, home_tabs);
        let active = std::mem::replace(&mut self.active, home_active);
        self.stored.insert(pane, StoredPane { tabs, active });
    }

    /// Scroll the pane under the pointer, whichever pane has focus, and leave the
    /// focus where it is. Returns `true` when the event was a wheel notch over a
    /// pane's content.
    ///
    /// Within the pane the notch goes to the part under the pointer: the Markdown
    /// preview beside the source, the commit view's file rail, or the document.
    /// Horizontal notches, and Shift with a vertical one, scroll columns over the
    /// document itself.
    pub(in crate::app) fn pane_wheel(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        let (delta, horizontal) = match mouse.kind {
            MouseEventKind::ScrollDown => (3, false),
            MouseEventKind::ScrollUp => (-3, false),
            MouseEventKind::ScrollRight => (3, true),
            MouseEventKind::ScrollLeft => (-3, true),
            _ => return false,
        };
        let Some((pane, in_editor, in_rail, in_preview)) = self
            .last_frame
            .panes
            .iter()
            .find(|frame| rect_contains(frame.content_rect, point))
            .map(|frame| {
                (
                    frame.pane,
                    rect_contains(frame.editor_rect, point),
                    rect_contains(frame.commit_rail_rect, point),
                    rect_contains(frame.markdown_preview_rect, point),
                )
            })
        else {
            return false;
        };
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        self.with_pane(pane, |app| {
            // The Seam view answers the wheel itself, from the hit map it recorded in
            // whichever pane painted it; the document scroll below has nothing to move.
            if app.seam_mouse(mouse) {
                return;
            }
            if horizontal {
                if in_editor {
                    app.scroll_columns(delta);
                }
            } else if in_preview {
                app.scroll_markdown_preview(delta);
            } else if in_rail {
                app.commit_rail_scroll(delta);
            } else if shift && in_editor {
                app.scroll_columns(delta);
            } else {
                app.scroll_lines(delta);
            }
        });
        true
    }

    /// Route a wheel notch or a press on a background pane's scrollbar. Returns
    /// `true` when it was one.
    ///
    /// The wheel scrolls that pane in place, as it does over its text. A press
    /// focuses the pane first, as a press on its text does, so a thumb drag it
    /// arms goes on writing to the pane it grabbed.
    pub(in crate::app) fn background_scrollbar_mouse(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        // Only the bars that scroll a pane's own tab; anything floating over a pane
        // (the outline overlay) is not the pane's to lend.
        let tab_scoped = self
            .last_frame
            .scroll
            .at(mouse.column, mouse.row)
            .is_some_and(|hit| {
                matches!(
                    hit.surface,
                    ScrollSurface::TabRows
                        | ScrollSurface::TabColumns
                        | ScrollSurface::EditorPreview
                        | ScrollSurface::CommitFileRail
                )
            });
        if !tab_scoped {
            return false;
        }
        let focused = self.focus_pane();
        let Some(pane) = self
            .last_frame
            .panes
            .iter()
            .find(|frame| frame.pane != focused && rect_contains(frame.content_rect, point))
            .map(|frame| frame.pane)
        else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(_) => {
                self.focus_pane_switch(pane);
                self.focus = Focus::Editor;
                self.scrollbar_mouse(mouse)
            },
            MouseEventKind::ScrollDown
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {
                let mut taken = false;
                self.with_pane(pane, |app| taken = app.scrollbar_mouse(mouse));
                taken
            },
            _ => false,
        }
    }

    /// The completion popup owns the pointer over it: a click on a row accepts
    /// that candidate, and the wheel scrolls the list rather than the text under
    /// it. Returns `true` when the event landed on the popup. Its scrollbar is
    /// routed before this, with every other bar.
    pub(in crate::app) fn handle_completion_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(list) = self.completion.popup.as_ref().map(|ui| ui.list) else {
            return false;
        };
        if !rect_contains(list.area(), (mouse.column, mouse.row)) {
            return false;
        }
        let delta = match mouse.kind {
            MouseEventKind::ScrollDown => 3,
            MouseEventKind::ScrollUp => -3,
            MouseEventKind::Down(MouseButton::Left) => {
                let len = self.completion_ranked().map_or(0, |ranked| ranked.len());
                if let Some(row) = list
                    .row_at(mouse.column, mouse.row)
                    .filter(|row| *row < len)
                {
                    if let Some(ui) = self.completion.popup.as_mut() {
                        ui.list.select(row, len);
                    }
                    self.accept_completion();
                }
                return true;
            },
            MouseEventKind::Down(_) => return true,
            _ => return false,
        };
        let len = self.completion_ranked().map_or(0, |ranked| ranked.len());
        if let Some(ui) = self.completion.popup.as_mut() {
            ui.list.scroll_by(delta, len);
        }
        true
    }

    /// The hover popup owns the pointer over it: the wheel scrolls content taller
    /// than the room it was given, and a press inside it is kept. Returns `true`
    /// when the event landed on it. A press anywhere else has already dismissed
    /// it (see [`dismiss_hover_on_press_outside`](Self::dismiss_hover_on_press_outside)).
    pub(in crate::app) fn handle_hover_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(ui) = self.hover_ui.as_mut() else {
            return false;
        };
        if !rect_contains(ui.rect, (mouse.column, mouse.row)) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => ui.scroll = ui.scroll.saturating_add(3),
            MouseEventKind::ScrollUp => ui.scroll = ui.scroll.saturating_sub(3),
            MouseEventKind::Down(_) => {},
            _ => return false,
        }
        true
    }

    /// Close the hover popup when a press lands anywhere outside it. The press
    /// then goes on to do what it would have done with no popup open.
    pub(in crate::app) fn dismiss_hover_on_press_outside(&mut self, mouse: MouseEvent) {
        if matches!(mouse.kind, MouseEventKind::Down(_))
            && self
                .hover_ui
                .as_ref()
                .is_some_and(|ui| !rect_contains(ui.rect, (mouse.column, mouse.row)))
        {
            self.hover_ui = None;
        }
    }

    /// The diagnostic detail view is modal for the pointer as it is for keys: the
    /// wheel scrolls it wherever the pointer is, a press outside it dismisses it,
    /// and nothing reaches the editor behind it. Returns `true` while it is open.
    pub(in crate::app) fn handle_diagnostic_view_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(view) = self.diagnostic_view.as_mut() else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::ScrollDown => view.scroll = view.scroll.saturating_add(3),
            MouseEventKind::ScrollUp => view.scroll = view.scroll.saturating_sub(3),
            MouseEventKind::Down(_) if !rect_contains(view.rect, (mouse.column, mouse.row)) => {
                self.diagnostic_view = None;
            },
            _ => {},
        }
        true
    }

    /// Before a right click opens the menu: move the caret to the click when it is
    /// outside every selection, so the menu's actions (Cut, Copy, Paste, a
    /// permalink) apply where the user pointed. A click inside a selection keeps
    /// it, as that is what the menu then acts on. `area` is the editor viewport of
    /// the pane the click landed in, already focused.
    pub(in crate::app) fn right_click_caret(&mut self, area: Rect, column: u16, row: u16) {
        if !rect_contains(area, (column, row)) {
            return;
        }
        let Some(Tab {
            kind:
                TabKind::Code {
                    buffer,
                    folds,
                    folded,
                    ..
                },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return;
        };
        let fold_lines = resolve_folds(folds, folded);
        let under = editor.char_pos_at(area, buffer, &fold_lines, column, row);
        if editor
            .selection_ranges()
            .iter()
            .any(|range| range.start <= under && under < range.end)
        {
            return;
        }
        let pos = editor.pos_at(area, buffer, &fold_lines, column, row);
        editor.set_caret(buffer, pos);
    }

    /// Handle mouse interaction with an open context menu.
    pub(in crate::app) fn handle_context_menu_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(menu) = self.context_menu.as_ref() else {
            return false;
        };
        let point = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if rect_contains(menu.rect(), point) => {
                // The click and the hover accent resolve rows the same way, so
                // the row that lit up is the row that runs.
                let Some(row) = menu.row_at(mouse.column, mouse.row) else {
                    self.accept_context_menu();
                    return true;
                };
                let picked = self
                    .context_menu
                    .as_mut()
                    .is_some_and(|menu| menu.select_row(row));
                if picked {
                    self.accept_context_menu();
                    return true;
                }
                // A disabled row keeps the cursor where it was (keyboard
                // navigation never rests on one either) but still owes the user
                // its note; the menu stays open so another row can be chosen.
                let note = self
                    .context_menu
                    .as_ref()
                    .and_then(|menu| menu.entries().get(row))
                    .and_then(|entry| entry.note.clone());
                if let Some(note) = note {
                    self.notify(Report::Refusal, NotificationKind::System, note);
                }
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

/// The text a toast's copy action puts on the clipboard: its title, then its
/// body on the next line when it has one.
pub(in crate::app) fn notification_clipboard_text(notification: &Notification) -> String {
    notification.body.as_ref().map_or_else(
        || notification.title.clone(),
        |body| format!("{}\n{body}", notification.title),
    )
}
