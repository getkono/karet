//! Who a press hands the keyboard to: the find bar's fields take it, and a press
//! outside the lightweight overlays settles them — split from the core mouse
//! module to respect the per-file code-line ceiling.

use crossterm::event::KeyModifiers;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;

use crate::app::App;
use crate::app::hit::TextFieldTarget;
use crate::app::hit::rect_contains;
use crate::tab::SearchField;

impl App {
    /// Let the lightweight overlays that hold the keyboard answer a press outside
    /// them, the way a GUI widget answers losing focus: an inline Explorer rename
    /// commits, the commit console hides (the commit keeps running), and the find
    /// bar stays open but hands the keys back. The press then goes on to whatever
    /// it landed on. Returns whether it landed on the open commit console.
    pub(super) fn settle_overlays_on_press(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return false;
        }
        let point = (mouse.column, mouse.row);
        let in_console = self.commit_console.open && rect_contains(self.commit_console.rect, point);
        if self.commit_console.open && !in_console {
            self.commit_console_dismiss();
        }
        if self.explorer.is_editing()
            && !self
                .explorer
                .edit_rect()
                .is_some_and(|rect| rect_contains(rect, point))
        {
            self.explorer_commit_edit();
        }
        let in_find_field = rect_contains(self.last_frame.find_rects().query, point)
            || self
                .last_frame
                .find_rects()
                .replace
                .is_some_and(|rect| rect_contains(rect, point));
        if self.find_open
            && !in_find_field
            && let Some(find) = self.active_find_mut()
        {
            find.focused = false;
        }
        in_console
    }

    /// Place the caret in the find bar's field under `(column, row)` and arm a
    /// selection drag; whether the click landed on one.
    pub(super) fn handle_find_bar_click(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        let target = if rect_contains(self.last_frame.find_rects().query, point) {
            TextFieldTarget::FindQuery
        } else if self
            .last_frame
            .find_rects()
            .replace
            .is_some_and(|r| rect_contains(r, point))
        {
            TextFieldTarget::FindReplace
        } else {
            return false;
        };
        let field = match target {
            TextFieldTarget::FindReplace => SearchField::Replace,
            _ => SearchField::Find,
        };
        // A field click is the bar taking the keyboard, wherever focus was —
        // including from the commit box, which would otherwise outrank it.
        self.focus_find(field);
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        self.place_text_field_cursor(target, mouse.column, mouse.row, shift);
        self.text_field_drag = Some(target);
        true
    }

    /// Put `target`'s text cursor at the cell under `(column, row)`, extending
    /// its selection when `extend` holds.
    pub(super) fn place_text_field_cursor(
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
            TextFieldTarget::ExplorerRename => {
                let Some(rect) = self.explorer.edit_rect() else {
                    return;
                };
                self.explorer
                    .edit_place_cursor(usize::from(column.saturating_sub(rect.x)), extend);
            },
            TextFieldTarget::FindQuery | TextFieldTarget::FindReplace => {
                let query = target == TextFieldTarget::FindQuery;
                let rect = if query {
                    Some(self.last_frame.find_rects().query)
                } else {
                    self.last_frame.find_rects().replace
                };
                let Some(rect) = rect else {
                    return;
                };
                let Some(find) = self.active_find_mut() else {
                    return;
                };
                let (text, edit) = if query {
                    (&mut find.query, &mut find.query_edit)
                } else {
                    (&mut find.replace, &mut find.replace_edit)
                };
                let cell = usize::from(column.saturating_sub(rect.x).saturating_add(edit.scroll));
                let cursor = karet_widgets::textfield::byte_at_cell(text, cell);
                edit.set_cursor(text, cursor, extend);
            },
            TextFieldTarget::SearchReplace
            | TextFieldTarget::SearchIncludes
            | TextFieldTarget::SearchExcludes => {
                let (rect, text, edit) = match target {
                    TextFieldTarget::SearchIncludes => (
                        self.search_ui.includes_rect,
                        &self.search.includes,
                        &mut self.search.includes_edit,
                    ),
                    TextFieldTarget::SearchExcludes => (
                        self.search_ui.excludes_rect,
                        &self.search.excludes,
                        &mut self.search.excludes_edit,
                    ),
                    _ => (
                        self.search_ui.replace_rect,
                        &self.search.replace,
                        &mut self.search.replace_edit,
                    ),
                };
                let Some(rect) = rect else {
                    return;
                };
                let cell = usize::from(column.saturating_sub(rect.x).saturating_add(edit.scroll));
                let cursor = karet_widgets::textfield::byte_at_cell(text, cell);
                let owned = text.clone();
                edit.set_cursor(&owned, cursor, extend);
            },
        }
    }
}
