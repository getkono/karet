//! Links written in an editor's source: Ctrl/Cmd-hover underlines them and
//! Ctrl/Cmd+click opens them, with no language server involved.

use karet_core::DecorationKind;

use super::support::*;
use crate::app::*;

/// A pointer event of `kind` at `(col, row)` carrying `mods`.
fn pointer(kind: MouseEventKind, col: u16, row: u16, mods: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: mods,
    }
}

/// An app editing `name` holding `text`, drawn in one focused pane at the
/// origin. A one-line file has a 3-cell gutter, so screen column `3 + c` is
/// buffer column `c`.
fn source_app(name: &str, text: &str) -> App {
    let mut app = app();
    app.push_tab(text_tab(name, text));
    app.focus = Focus::Editor;
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 60, 5))];
    app
}

/// The range the hover at buffer column `col` of line 0 underlines, with `mods`.
fn underlined(app: &mut App, col: u16, mods: KeyModifiers) -> Option<(u32, u32)> {
    app.update_definition_hover(&pointer(MouseEventKind::Moved, 3 + col, 0, mods));
    let decoration = app.definition_underline_decoration()?;
    assert_eq!(decoration.kind, DecorationKind::Underline);
    Some((decoration.range.start.col, decoration.range.end.col))
}

#[test]
fn ctrl_hover_underlines_a_url_in_any_file_without_a_language_server() {
    // `// see ` is 7 characters, and the URL is 21.
    let mut app = source_app("main.rs", "// see https://example.com/a.\n");

    assert_eq!(
        underlined(&mut app, 10, KeyModifiers::CONTROL),
        Some((7, 28))
    );
    assert_eq!(underlined(&mut app, 7, KeyModifiers::SUPER), Some((7, 28)));
    assert_eq!(
        underlined(&mut app, 10, KeyModifiers::NONE),
        None,
        "no modifier"
    );
    assert_eq!(
        underlined(&mut app, 10, KeyModifiers::CONTROL | KeyModifiers::ALT),
        None,
        "Ctrl+Alt+click is the editor's, so it promises no link"
    );
    assert_eq!(
        underlined(&mut app, 28, KeyModifiers::CONTROL),
        None,
        "the trimmed sentence period is not part of the link"
    );
}

#[test]
fn ctrl_hover_underlines_a_whole_markdown_link_only_in_markdown() {
    let text = "see [the guide](docs/guide.md) now\n";
    let mut markdown = source_app("README.md", text);
    // On the text, and on the destination: `[` is column 4, `)` column 29.
    assert_eq!(
        underlined(&mut markdown, 6, KeyModifiers::CONTROL),
        Some((4, 30))
    );
    assert_eq!(
        underlined(&mut markdown, 25, KeyModifiers::CONTROL),
        Some((4, 30))
    );
    assert_eq!(underlined(&mut markdown, 1, KeyModifiers::CONTROL), None);

    // The bracket syntax means nothing outside Markdown.
    let mut rust = source_app("main.rs", text);
    assert_eq!(underlined(&mut rust, 6, KeyModifiers::CONTROL), None);
}

#[test]
fn ctrl_click_opens_a_markdown_source_link_to_a_workspace_file() {
    let root = test_dir("source-link-open");
    write_file(&root, "README.md", b"see [guide](docs/guide.md)\n");
    write_file(&root, "docs/guide.md", b"guide");
    let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
    app.open_path(&root.join("README.md"));
    app.focus = Focus::Editor;
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 60, 5))];
    let down = MouseEventKind::Down(MouseButton::Left);

    // A plain click and a Ctrl+Shift click stay the editor's.
    for mods in [
        KeyModifiers::NONE,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ] {
        assert!(!app.handle_markdown_link_mouse(pointer(down, 9, 0, mods)));
    }
    // Off the link, Ctrl+click is left for Go to Definition.
    assert!(!app.handle_markdown_link_mouse(pointer(down, 4, 0, KeyModifiers::CONTROL)));
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("README.md").as_path())
    );

    assert!(app.handle_markdown_link_mouse(pointer(down, 9, 0, KeyModifiers::CONTROL)));
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("docs/guide.md").as_path())
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ctrl_click_on_an_unsafe_source_link_is_blocked_not_opened() {
    let root = test_dir("source-link-blocked");
    write_file(&root, "README.md", b"[run](javascript:alert)\n");
    let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
    app.open_path(&root.join("README.md"));
    app.focus = Focus::Editor;
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 60, 5))];

    let click = pointer(
        MouseEventKind::Down(MouseButton::Left),
        5,
        0,
        KeyModifiers::CONTROL,
    );
    assert!(app.handle_markdown_link_mouse(click));
    assert!(
        app.notifications
            .active()
            .iter()
            .any(|note| note.title.starts_with("link blocked")),
        "the refusal is reported"
    );
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("README.md").as_path())
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ctrl_click_on_a_source_link_in_an_unfocused_split_opens_it_there() {
    let root = test_dir("source-link-split");
    write_file(&root, "README.md", b"[guide](docs/guide.md)\n");
    write_file(&root, "docs/guide.md", b"guide");
    let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
    app.open_path(&root.join("README.md"));
    app.focus = Focus::Editor;
    let left = app.focus_pane();
    app.dispatch(Command::SplitRight);
    assert_ne!(app.focus_pane(), left, "the new split is focused");
    let buffer = frame(&mut app, 120, 30);

    // The `[` the left split draws on its first text row.
    let Some(area) = app
        .pane_frames
        .iter()
        .find(|frame| frame.pane == left)
        .map(|frame| frame.editor_rect)
    else {
        panic!("the left split was not drawn");
    };
    let Some(column) = (area.left()..area.right()).find(|&x| buffer[(x, area.y)].symbol() == "[")
    else {
        panic!("the left split shows no link");
    };

    // On the link text, while the other split is focused: the link must be
    // read from the split under the pointer, not from the focused one.
    app.handle_mouse(pointer(
        MouseEventKind::Down(MouseButton::Left),
        column + 2,
        area.y,
        KeyModifiers::CONTROL,
    ));
    assert_eq!(app.focus_pane(), left, "the link's split is focused");
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("docs/guide.md").as_path()),
        "the link opened rather than falling through to Go to Definition"
    );
    let _ = std::fs::remove_dir_all(root);
}
