//! The inline rename prompt (F2), floating under the symbol it renames.

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::tab::TabKind;

/// The prompt's narrowest width, borders included: room for a short name and
/// the title.
const MIN_WIDTH: u16 = 24;

/// Draw the rename prompt anchored at the symbol: on the line below it when
/// there is room, so the name being replaced stays visible, else above it.
pub(super) fn draw_rename(f: &mut Frame, app: &mut App, theme: &Theme) {
    let editor = app.editor_rect;
    let Some(prompt) = app.rename.as_ref().filter(|prompt| prompt.shown) else {
        return;
    };
    if editor.width < 8 || editor.height < 3 {
        return;
    }
    let anchor = {
        let Some(tab) = app.tabs.get(app.active) else {
            return;
        };
        let TabKind::Code {
            buffer,
            folds,
            folded,
            ..
        } = &tab.kind
        else {
            return;
        };
        let fold_lines = crate::app::resolve_folds(folds, folded);
        tab.editor
            .screen_cell(editor, buffer, &fold_lines, prompt.range.start)
            .or_else(|| tab.editor.primary_caret_cell(editor, buffer, &fold_lines))
    };
    // A symbol scrolled out of view still gets its prompt, at the top.
    let (x, y) = anchor.unwrap_or((editor.left(), editor.top()));
    let text_width = u16::try_from(prompt.text.width()).unwrap_or(u16::MAX);
    let width = text_width
        .saturating_add(4)
        .max(MIN_WIDTH)
        .min(editor.width);
    let below = editor.bottom().saturating_sub(y.saturating_add(1));
    let top = if below >= 3 {
        y.saturating_add(1)
    } else {
        y.saturating_sub(3).max(editor.top())
    };
    let left = x.min(editor.right().saturating_sub(width));
    let rect = Rect::new(left, top, width, 3).intersection(editor);
    if rect.height < 3 {
        return;
    }
    let background = Style::default()
        .bg(theme.role(ThemeRole::Background).to_ratatui())
        .fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let border = background.fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(" Rename ")
        .style(background);
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    let Some(prompt) = app.rename.as_mut() else {
        return;
    };
    prompt.edit.ensure_cursor_visible(&prompt.text, inner.width);
    let selection = background.bg(theme.role(ThemeRole::Selection).to_ratatui());
    let caret = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    f.render_widget(
        Paragraph::new(super::text_field_text(
            &prompt.text,
            &prompt.edit,
            true,
            background,
            selection,
            caret,
        ))
        .scroll((0, prompt.edit.scroll)),
        inner,
    );
}
