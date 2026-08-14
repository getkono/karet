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

/// A left press at `(x, y)`.
fn press(app: &mut App, x: u16, y: u16) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

/// A left-button drag to `(x, y)`.
fn drag(app: &mut App, x: u16, y: u16) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

/// A wheel notch at `(x, y)`; `down` picks the direction.
fn wheel(app: &mut App, x: u16, y: u16, down: bool) {
    app.handle_mouse(MouseEvent {
        kind: if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        },
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

#[test]
fn a_wheel_notch_over_the_track_moves_one_line_while_the_text_moves_three() {
    // The requirement, asserted as a pair: the bar is the app's only fine scroll, so
    // neither half is allowed to regress without the other noticing.
    let mut app = app();
    app.push_tab(text_tab("tall.rs", &"line\n".repeat(200)));
    screen(&mut app, 40, 12);
    let rect = app.editor_rect;

    wheel(&mut app, rect.right(), rect.y + 1, true);
    assert_eq!(
        app.tabs[app.active].editor.scroll_line, 1,
        "a notch over the track should step exactly one line"
    );

    app.tabs[app.active].editor.scroll_line = 0;
    screen(&mut app, 40, 12);
    wheel(&mut app, rect.x + 1, rect.y + 1, true);
    assert_eq!(
        app.tabs[app.active].editor.scroll_line, 3,
        "a notch over the text should keep its three lines"
    );
}

#[test]
fn the_wheel_over_a_track_stops_at_the_ends() {
    let mut app = app();
    app.push_tab(text_tab("tall.rs", &"line\n".repeat(200)));
    screen(&mut app, 40, 12);
    let rect = app.editor_rect;

    wheel(&mut app, rect.right(), rect.y + 1, false);
    assert_eq!(app.tabs[app.active].editor.scroll_line, 0);
}

#[test]
fn dragging_the_thumb_scrolls_the_editor_and_reaches_the_last_line() {
    let mut app = app();
    app.push_tab(text_tab("tall.rs", &"line\n".repeat(200)));
    screen(&mut app, 40, 12);
    let rect = app.editor_rect;
    let track_x = rect.right();
    let hit = app
        .scroll_hits
        .at(track_x, rect.y)
        .expect("the editor's track should be registered");
    let (start, _) = hit.track.thumb_span().unwrap_or_default();

    // Grabbing without moving must not shift the view by so much as a line.
    press(&mut app, track_x, rect.y + start);
    assert!(app.scroll_drag.is_some(), "the press should grab the thumb");
    assert_eq!(app.tabs[app.active].editor.scroll_line, 0);

    // Dragging to the bottom of the track reaches the end of the document.
    drag(&mut app, track_x, rect.bottom());
    let visible = app.tabs[app.active].editor.visible_lines();
    assert_eq!(
        app.tabs[app.active].editor.scroll_line,
        201 - visible,
        "dragging to the end should land on the last position"
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: track_x,
        row: rect.bottom(),
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.scroll_drag.is_none(), "release should end the drag");
}

#[test]
fn a_drag_keeps_scrolling_after_the_pointer_leaves_the_track() {
    // A track is one column wide; a drag that wanders off it must not be dropped.
    let mut app = app();
    app.push_tab(text_tab("tall.rs", &"line\n".repeat(200)));
    screen(&mut app, 40, 12);
    let rect = app.editor_rect;
    let hit = app
        .scroll_hits
        .at(rect.right(), rect.y)
        .expect("the editor's track should be registered");
    let (start, _) = hit.track.thumb_span().unwrap_or_default();

    press(&mut app, rect.right(), rect.y + start);
    drag(&mut app, 0, rect.y + start + 3);

    assert!(
        app.scroll_drag.is_some(),
        "the drag should still be captured"
    );
    assert!(
        app.tabs[app.active].editor.scroll_line > 0,
        "a drag off the track should still scroll"
    );
}

#[test]
fn clicking_the_groove_below_the_thumb_pages_down() {
    let mut app = app();
    app.push_tab(text_tab("tall.rs", &"line\n".repeat(200)));
    screen(&mut app, 40, 12);
    let rect = app.editor_rect;

    press(&mut app, rect.right(), rect.bottom() - 1);

    let visible = app.tabs[app.active].editor.visible_lines();
    assert_eq!(app.tabs[app.active].editor.scroll_line, visible);
    assert!(
        app.scroll_drag.is_none(),
        "a groove click pages; it does not start a drag"
    );
}

#[test]
fn a_track_for_content_that_fits_is_inert_and_still_swallows_the_click() {
    // The column belongs to the bar even when no thumb is painted, so a press there
    // must neither scroll nor fall through to the text beside it.
    let mut app = app();
    app.push_tab(text_tab("short.rs", "one\ntwo\nthree\n"));
    screen(&mut app, 40, 12);
    let rect = app.editor_rect;

    assert!(
        app.scroll_hits.at(rect.right(), rect.y).is_none(),
        "a suppressed bar should not be registered"
    );
    press(&mut app, rect.right(), rect.y + 2);
    assert_eq!(app.tabs[app.active].editor.scroll_line, 0);
    assert!(app.scroll_drag.is_none());
}

#[test]
fn the_registry_is_rebuilt_every_frame() {
    // Last-frame geometry, like every other hit region: a frame that no longer paints
    // a bar must not leave a grabbable ghost of it behind.
    let mut app = app();
    app.push_tab(text_tab("tall.rs", &"line\n".repeat(200)));
    screen(&mut app, 40, 12);
    assert!(app.scroll_hits.of(ScrollSurface::TabRows).is_some());

    app.tabs[app.active] = text_tab("short.rs", "one\n");
    screen(&mut app, 40, 12);
    assert!(
        app.scroll_hits.of(ScrollSurface::TabRows).is_none(),
        "the previous frame's track should not survive"
    );
}

#[test]
fn dragging_the_explorer_thumb_scrolls_the_tree_without_it_snapping_back() {
    // The explorer's offset is pinned to its cursor by the render, so an offset
    // written on its own would be undone before it was ever seen. This is the guard
    // for the whole cursor-driven family of surfaces.
    let dir = test_dir("scrollbar-explorer");
    for i in 0..60 {
        write_file(&dir, &format!("file-{i:02}.txt"), b"x");
    }
    let mut app = App::new(dir, Vec::new(), Vec::new(), false);
    app.sidebar_visible = true;
    app.sidebar_panel = SidebarPanel::Explorer;
    screen(&mut app, 60, 20);

    let hit = app
        .scroll_hits
        .of(ScrollSurface::Explorer)
        .expect("a 60-row tree in a 20-row sidebar should paint a bar");
    let track = hit.track.rect();
    let (start, _) = hit.track.thumb_span().unwrap_or_default();

    press(&mut app, track.x, track.y + start);
    drag(&mut app, track.x, track.bottom() - 1);
    let dragged = app.explorer.offset();
    assert!(dragged > 0, "the drag should have scrolled the tree");

    // The next frame is where a naive offset write would be undone.
    screen(&mut app, 60, 20);
    assert_eq!(
        app.explorer.offset(),
        dragged,
        "the tree scrolled back on the next frame"
    );
}

#[test]
fn the_sidebar_track_scrolls_the_sidebar_and_not_the_editor() {
    // The routing the narrowed hit rects made possible: the bar's column sits inside
    // `sidebar_rect`, so without the registry the click would reach the panel.
    let dir = test_dir("scrollbar-routing");
    for i in 0..60 {
        write_file(&dir, &format!("file-{i:02}.txt"), b"x");
    }
    let mut app = App::new(dir, Vec::new(), Vec::new(), false);
    app.sidebar_visible = true;
    app.sidebar_panel = SidebarPanel::Explorer;
    app.push_tab(text_tab("tall.rs", &"line\n".repeat(200)));
    screen(&mut app, 60, 20);

    let track = app
        .scroll_hits
        .of(ScrollSurface::Explorer)
        .expect("the explorer should paint a bar")
        .track
        .rect();
    wheel(&mut app, track.x, track.y + 1, true);

    assert_eq!(
        app.tabs[app.active].editor.scroll_line, 0,
        "a notch over the sidebar's bar must not scroll the editor"
    );
}
