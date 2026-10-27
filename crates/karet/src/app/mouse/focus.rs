//! Who a press hands the keyboard to: the find bar's fields take it, and a press
//! outside the lightweight overlays settles them — split from the core mouse
//! module to respect the per-file code-line ceiling.

use super::*;

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
        if let Some(find) = self.active_find_mut() {
            find.field = field;
            find.focused = true;
        }
        // A field click is the bar taking the keyboard, wherever focus was —
        // including from the commit box, which would otherwise outrank it.
        self.focus = Focus::Editor;
        self.commit_input.focused = false;
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        self.place_text_field_cursor(target, mouse.column, mouse.row, shift);
        self.text_field_drag = Some(target);
        true
    }
}
