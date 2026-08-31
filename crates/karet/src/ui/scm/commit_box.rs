//! The commit-message box: a bordered, growing, scrolling draft editor.
//!
//! The box owns three things the rest of the panel does not care about: the
//! border and its AI chip, the text area inside it, and the scrollbar that
//! appears once the draft outgrows the box. Its height is decided by
//! [`commit_box_height`](crate::app::scm::commit_box::commit_box_height) before
//! the panel is split, so everything here draws into a rect that is already the
//! right size.

use karet_widgets::textarea::TextArea;
use karet_widgets::textarea::TextAreaStyle;
use karet_widgets::textarea::wrap_rows;

use super::*;

/// Draw the commit editor into `area`, including its border, AI chip and
/// scrollbar, and record where its text landed for hit-testing.
pub(super) fn draw_commit_input(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    if area.width == 0 || area.height == 0 {
        app.scm_ui.commit_rect = Rect::default();
        return;
    }
    let accent = theme.role(ThemeRole::LineNumberActive).to_ratatui();
    let muted = theme.role(ThemeRole::LineNumber).to_ratatui();
    // Short on purpose. The sidebar defaults to 30 columns, and the old
    // "· Ctrl+Enter commit" tail was wider than the whole panel — ratatui
    // truncated it away unseen, while still costing the border room the AI chip
    // needs. The commit chord lives in the status hints bar, which is where the
    // rest of the context-sensitive keys are advertised.
    let title = if app.commit_input.pending.is_some() {
        " Commit message · committing… "
    } else {
        " Commit message "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if app.commit_input.focused {
            accent
        } else {
            muted
        }));
    let inner = block.inner(area);
    f.render_widget(block, area);
    super::draw_ai_chip(f, app, theme, area, title);
    if inner.width == 0 || inner.height == 0 {
        app.scm_ui.commit_rect = Rect::default();
        return;
    }

    // Reserve before wrapping, never after: the track's column is part of the
    // width the draft wraps to. The commit rect is the *content* rect, so a click
    // on the track cannot be mistaken for a click in the text.
    let (content, tracks) = reserve_tracks(inner, ScrollAxes::VERTICAL);
    app.scm_ui.commit_rect = content;
    if content.width == 0 || content.height == 0 {
        return;
    }

    if app.commit_input.scrolled_away {
        app.commit_input
            .edit
            .clamp_scroll(&app.commit_input.text, content.width, content.height);
    } else {
        app.commit_input.edit.ensure_cursor_visible(
            &app.commit_input.text,
            content.width,
            content.height,
        );
    }
    let foreground = theme.role(ThemeRole::Foreground).to_ratatui();
    let selection = theme.role(ThemeRole::Selection).to_ratatui();
    f.render_widget(
        TextArea::new(&app.commit_input.text, &app.commit_input.edit)
            .focused(app.commit_input.focused)
            .style(TextAreaStyle::new(
                Style::default().fg(foreground),
                Style::default().fg(foreground).bg(selection),
                Style::default().fg(accent),
            ))
            .placeholder("Type a commit message", Style::default().fg(muted)),
        content,
    );
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(
                wrap_rows(&app.commit_input.text, content.width).len(),
                app.commit_input.edit.scroll.into(),
                content.height.into(),
            ),
            ScrollExtent::default(),
        ),
        ScrollSurface::ScmCommitInput,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::scm::commit_box::MAX_COMMIT_ROWS;
    use crate::app::scm::commit_box::MIN_COMMIT_ROWS;
    use crate::app::scm::commit_box::commit_box_height;

    /// Draw the box for `draft` into a sidebar-width terminal, returning the
    /// painted rows and the box's own height.
    fn draw(draft: &str, height: u16) -> (Vec<String>, u16, Rect) {
        let mut app = crate::app::tests::support::app();
        app.commit_input.text = draft.to_string();
        app.commit_input.focused = true;
        let theme = app.theme.clone();
        let area = Rect::new(0, 0, 30, height);
        let box_height = commit_box_height(area, draft);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .unwrap_or_else(|_| unreachable!("the test backend is infallible"));
        let mut hits = ScrollHits::default();
        let _ = terminal.draw(|frame| {
            draw_commit_input(
                frame,
                &mut app,
                &theme,
                Rect {
                    height: box_height,
                    ..area
                },
                &mut hits,
            );
        });
        let buffer = terminal.backend().buffer().clone();
        let rows = (0..box_height)
            .map(|y| {
                (0..area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                    .collect()
            })
            .collect();
        (rows, box_height, app.scm_ui.commit_rect)
    }

    #[test]
    fn a_short_draft_paints_inside_the_box_it_always_had() {
        let (rows, height, content) = draw("fix the thing", 40);
        assert_eq!(height, MIN_COMMIT_ROWS + 2);
        assert!(rows[1].contains("fix the thing"), "{:?}", rows[1]);
        // The content rect excludes the border *and* the scrollbar column, so a
        // click on the track is not a click in the text.
        assert_eq!(content.width, 30 - 2 - 1);
        assert_eq!(content.height, MIN_COMMIT_ROWS);
    }

    #[test]
    fn a_growing_draft_takes_more_rows_until_the_cap() {
        let (_, height, _) = draw("a\nb\nc\nd\ne", 40);
        assert_eq!(height, 5 + 2, "five lines, five rows");
        let long = "x\n".repeat(40);
        let (_, height, content) = draw(&long, 40);
        assert_eq!(height, MAX_COMMIT_ROWS + 2);
        assert_eq!(content.height, MAX_COMMIT_ROWS);
    }

    #[test]
    fn a_draft_past_the_cap_paints_a_scrollbar_thumb() {
        let long = "x\n".repeat(40);
        let (rows, height, _) = draw(&long, 40);
        // The track is the last column of the box's interior.
        let track: String = rows[1..usize::from(height) - 1]
            .iter()
            .filter_map(|row| row.chars().nth(usize::from(28u16)))
            .collect();
        assert!(
            track.chars().any(|glyph| glyph != ' '),
            "a draft twice the box's height must show a thumb: {track:?}"
        );

        // A draft that fits leaves the column empty, though it stays reserved.
        let (rows, height, _) = draw("short", 40);
        let track: String = rows[1..usize::from(height) - 1]
            .iter()
            .filter_map(|row| row.chars().nth(usize::from(28u16)))
            .collect();
        assert!(track.chars().all(|glyph| glyph == ' '), "{track:?}");
    }

    #[test]
    fn a_long_line_wraps_inside_the_box_rather_than_running_off_it() {
        let (rows, _, content) = draw("the quick brown fox jumps over the lazy dog", 40);
        assert!(rows[1].contains("the quick"), "{:?}", rows[1]);
        assert!(
            rows[2].trim_end().len() > 1,
            "the draft continues: {:?}",
            rows[2]
        );
        // Nothing is painted past the text width.
        for row in &rows[1..4] {
            let painted: String = row.chars().take(usize::from(content.width) + 1).collect();
            assert!(painted.ends_with(' ') || painted.len() <= usize::from(content.width) + 1);
        }
    }
}
