//! Painting the confirmation dialog.
//!
//! The widget owns centering, wrapping and the selection accent; this module
//! resolves what each row *says* — the label the caller gave it, or the label of
//! the command behind it — and the key hint that runs it.

use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::App;
use crate::app::confirm::ConfirmAction;
use crate::app::confirm::confirm_label;
use crate::keymap;
use crate::keymap::ChordStyle;

/// Draw the open confirmation dialog, if there is one.
pub(super) fn draw_confirm(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let Some(dialog) = app.confirm.as_mut() else {
        return;
    };
    let labels: Vec<String> = dialog
        .choices
        .entries
        .iter()
        .map(|entry| {
            entry
                .label
                .clone()
                .unwrap_or_else(|| confirm_label(&entry.action))
        })
        .collect();
    // Only the two navigation-free answers carry a hint: Esc always cancels, and
    // Enter always runs the selected row, so hinting every row would be noise.
    let hints: Vec<Option<String>> = dialog
        .choices
        .entries
        .iter()
        .map(|entry| match entry.action {
            ConfirmAction::Cancel => {
                keymap::hint_for(crate::command::Command::ConfirmCancel, ChordStyle::Caret)
            },
            _ => None,
        })
        .collect();
    dialog.draw(f, theme, area, &labels, &hints);
}
