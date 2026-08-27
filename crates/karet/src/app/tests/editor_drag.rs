//! The editor's pointer selection drag: word and line granularity, and the
//! autoscroll that keeps a drag alive past the edge of the viewport.

use super::support::*;
use crate::app::*;

/// An app with `text` in a code tab and one focused pane covering `area`.
fn drag_app(text: &str, area: Rect) -> App {
    let mut app = app();
    app.push_tab(text_tab("t.rs", text));
    app.pane_frames = vec![content_frame(&app, area)];
    app.editor_rect = area;
    app
}

fn press(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn a_double_click_drag_extends_by_whole_words() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 5,
    };
    let mut app = drag_app("foo bar baz qux", area);
    // Double-click "bar" (buffer col 5 -> screen col 8), then drag into "baz".
    app.handle_editor_click(press(8, 0));
    app.handle_editor_click(press(8, 0));
    app.drag_select_to(12, 0);

    assert_eq!(
        app.tabs[app.active].editor.selection_range(),
        Some(Range {
            start: LineCol::new(0, 4),
            end: LineCol::new(0, 11),
        }),
        "the drag swallows whole words, never a partial one"
    );
}

#[test]
fn a_double_click_drag_backwards_keeps_the_word_it_started_on() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 5,
    };
    let mut app = drag_app("foo bar baz qux", area);
    // Double-click "baz" (buffer col 9 -> screen col 12), then drag back into "foo".
    app.handle_editor_click(press(12, 0));
    app.handle_editor_click(press(12, 0));
    app.drag_select_to(4, 0);

    assert_eq!(
        app.tabs[app.active].editor.selection_range(),
        Some(Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 11),
        }),
        "dragging back keeps the opening word and adds whole words before it"
    );
}

#[test]
fn a_triple_click_drag_extends_by_whole_lines() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 5,
    };
    let mut app = drag_app("alpha\nbravo\ncharlie\n", area);
    for _ in 0..3 {
        app.handle_editor_click(press(8, 0));
    }
    app.drag_select_to(9, 1);

    let sel = app.tabs[app.active].editor.selection_range();
    assert_eq!(
        sel.map(|range| (range.start.line, range.end.line)),
        Some((0, 1)),
        "a line drag covers both lines end to end"
    );
    assert_eq!(sel.map(|range| range.start.col), Some(0));
}

#[test]
fn a_single_click_drag_still_extends_character_by_character() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 5,
    };
    let mut app = drag_app("foo bar baz", area);
    app.handle_editor_click(press(8, 0));
    app.drag_select_to(10, 0);

    assert_eq!(
        app.tabs[app.active].editor.selection_range(),
        Some(Range {
            start: LineCol::new(0, 5),
            end: LineCol::new(0, 7),
        }),
        "a plain drag lands on the exact characters under the pointer"
    );
}

#[test]
fn dragging_below_the_viewport_scrolls_and_keeps_extending() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 4,
    };
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut app = drag_app(&text, area);
    app.handle_editor_click(press(8, 0));

    // Park the pointer one row below the viewport and let the loop tick.
    app.drag_select_to(8, area.bottom());
    let before = app.tabs[app.active].editor.scroll_line;
    app.tick_drag_autoscroll();
    let after = app.tabs[app.active].editor.scroll_line;
    assert!(after > before, "the viewport creeps toward the pointer");
    assert!(
        app.tabs[app.active]
            .editor
            .selection_range()
            .is_some_and(|range| range.end.line > 0),
        "the selection follows the rows scrolling into view"
    );

    // The loop is told to come back for the next step.
    assert!(app.drag_autoscroll_wake().is_some());

    // Releasing the button ends both the drag and the autoscroll.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 8,
        row: area.bottom(),
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.editor_drag.is_none());
    assert_eq!(app.drag_autoscroll_wake(), None);
}

#[test]
fn a_drag_inside_the_viewport_never_autoscrolls() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 6,
    };
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut app = drag_app(&text, area);
    app.handle_editor_click(press(4, 0));
    app.drag_select_to(4, 2);

    assert_eq!(app.drag_autoscroll_wake(), None);
    let before = app.tabs[app.active].editor.scroll_line;
    app.tick_drag_autoscroll();
    assert_eq!(app.tabs[app.active].editor.scroll_line, before);
}
