//! Scrollbar geometry at the shell level: that reserving a track keeps what the
//! editor *paints* and what a click *resolves to* in agreement.

use super::support::*;
use crate::app::*;

/// The thumb glyph ratatui paints.
const THUMB: &str = "\u{2588}";

/// A long single line, so the editor overflows horizontally but not vertically.
fn long_line() -> String {
    ('a'..='z').cycle().take(300).collect()
}

#[test]
fn a_click_on_the_last_text_column_resolves_to_the_character_drawn_there() {
    // The guard for the whole reservation scheme: `editor_rect` is both the rect the
    // widget paints into and the rect `pos_at` maps clicks against, so if reserving
    // the track ever shrank only one of them, every click would land one column off.
    let text = long_line();
    let mut app = app();
    app.push_tab(text_tab("wide.rs", &text));
    let rows = screen(&mut app, 40, 12);

    let rect = app.editor_rect;
    let (x, y) = (rect.right() - 1, rect.y);
    app.handle_editor_click(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });

    let caret = app.tabs[app.active].editor.cursor();
    let drawn = rows
        .get(y as usize)
        .and_then(|row| row.chars().nth(x as usize));
    let under_caret = text.chars().nth(caret.col as usize);
    assert_eq!(
        drawn, under_caret,
        "clicking ({x}, {y}) put the caret at {caret:?}, which is not the cell drawn there"
    );
}

#[test]
fn the_editor_paints_its_bars_outside_the_text() {
    // One long line in a short pane: horizontal overflows, vertical does not.
    let mut app = app();
    app.push_tab(text_tab("wide.rs", &long_line()));
    let rows = screen(&mut app, 40, 12);

    let rect = app.editor_rect;
    let track_x = rect.right();
    let track_y = rect.bottom();
    assert!(track_x < 40, "no vertical track was reserved");

    let bottom = rows.get(track_y as usize).map(String::as_str).unwrap_or("");
    assert!(
        bottom.contains(THUMB),
        "horizontal overflow should paint a bar in the reserved row, got {bottom:?}"
    );
    // A single line cannot overflow the pane's height, so the vertical bar is
    // suppressed — while its column stays reserved either way.
    let vertical: String = rows
        .iter()
        .filter_map(|row| row.chars().nth(track_x as usize))
        .collect();
    assert!(
        !vertical.contains(THUMB),
        "content that fits should not paint a vertical bar, got {vertical:?}"
    );
}

#[test]
fn a_tall_document_paints_a_vertical_bar_that_tracks_the_scroll() {
    let text = "line\n".repeat(200);
    let mut app = app();
    app.push_tab(text_tab("tall.rs", &text));
    let rows = screen(&mut app, 40, 12);
    let track_x = app.editor_rect.right();

    let thumb_rows = |rows: &[String]| -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| row.chars().nth(track_x as usize) == THUMB.chars().next())
            .map(|(y, _)| y)
            .collect()
    };
    let at_top = thumb_rows(&rows);
    assert!(!at_top.is_empty(), "a 200-line file should show a thumb");

    app.tabs[app.active].editor.scroll_line = 188;
    let scrolled = thumb_rows(&screen(&mut app, 40, 12));
    assert!(
        scrolled.first() > at_top.first(),
        "scrolling down should move the thumb down: {at_top:?} then {scrolled:?}"
    );
}
