//! Painting the confirmation dialog.
//!
//! The widget owns centering, wrapping and the selection accent; this module
//! resolves what each row *says* — the label the caller gave it, or the label of
//! the command behind it — and the key hint that runs it.

use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::App;
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
    // Exactly one row carries a hint: the first, because backing out *is* taking
    // it. Keying that off the `Cancel` variant would have missed the two dialogs
    // where it matters most — the close prompt and the crash-recovery prompt put
    // a cleanup command in row zero, so they showed no way out at all.
    let hint = keymap::hint_for(crate::command::Command::ConfirmCancel, ChordStyle::Caret);
    let hints: Vec<Option<String>> = (0..dialog.choices.entries.len())
        .map(|row| (row == 0).then(|| hint.clone()).flatten())
        .collect();
    dialog.draw(f, theme, area, &labels, &hints);
}
