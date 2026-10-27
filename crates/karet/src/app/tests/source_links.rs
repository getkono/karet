//! Links written in an editor's source: Ctrl/Cmd-hover underlines them and
//! Ctrl/Cmd+click opens them, with no language server involved.

use karet_core::DecorationKind;
use karet_core::FoldRegion;
use karet_core::FoldRegions;

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
    app.keyboard.focus_editor();
    app.last_frame.panes = vec![content_frame(&app, Rect::new(0, 0, 60, 5))];
    app
}

/// The range the hover at buffer column `col` of line 0 underlines, with `mods`.
fn underlined(app: &mut App, col: u16, mods: KeyModifiers) -> Option<(u32, u32)> {
    app.update_definition_hover(&pointer(MouseEventKind::Moved, 3 + col, 0, mods));
    let decoration = app.definition_underline_decoration()?;
    assert_eq!(decoration.kind, DecorationKind::Underline);
    Some((decoration.range.start.col, decoration.range.end.col))
}

/// The glyph a cell's `symbol` shows, without the OSC 8 hyperlink a link's
/// cells are wrapped in.
fn glyph(symbol: &str) -> &str {
    if !symbol.starts_with("\u{1b}]8;") {
        return symbol;
    }
    symbol
        .split_once("\u{1b}\\")
        .and_then(|(_, rest)| rest.split_once('\u{1b}'))
        .map_or(symbol, |(glyph, _)| glyph)
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
    fill_reserved_tabs(&mut app);
    app.keyboard.focus_editor();
    app.last_frame.panes = vec![content_frame(&app, Rect::new(0, 0, 60, 5))];
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
    fill_reserved_tabs(&mut app);
    app.keyboard.focus_editor();
    app.last_frame.panes = vec![content_frame(&app, Rect::new(0, 0, 60, 5))];

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
    fill_reserved_tabs(&mut app);
    app.keyboard.focus_editor();
    let left = app.focus_pane();
    app.dispatch(Command::SplitRight);
    assert_ne!(app.focus_pane(), left, "the new split is focused");
    let buffer = frame(&mut app, 120, 30);

    // The `[` the left split draws on its first text row (wrapped in its OSC 8
    // hyperlink, as every visible source link is).
    let Some(area) = app
        .last_frame
        .panes
        .iter()
        .find(|frame| frame.pane == left)
        .map(|frame| frame.editor_rect)
    else {
        panic!("the left split was not drawn");
    };
    let Some(column) =
        (area.left()..area.right()).find(|&x| glyph(buffer[(x, area.y)].symbol()) == "[")
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

/// An app rooted in a fresh `name` directory holding `README.md` with
/// `readme`, plus `docs/guide.md`, with the README open and focused.
fn workspace_app(name: &str, readme: &[u8]) -> (App, std::path::PathBuf) {
    let root = test_dir(name);
    write_file(&root, "README.md", readme);
    write_file(&root, "docs/guide.md", b"guide");
    let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
    app.open_path(&root.join("README.md"));
    fill_reserved_tabs(&mut app);
    app.keyboard.focus_editor();
    (app, root)
}

/// The path of the active tab's file.
fn active_path(app: &App) -> Option<std::path::PathBuf> {
    app.tabs
        .get(app.active)?
        .path()
        .map(std::path::Path::to_path_buf)
}

#[test]
fn open_link_at_caret_opens_the_link_the_caret_is_in() {
    let (mut app, root) = workspace_app("source-link-caret", b"see [guide](docs/guide.md) now\n");
    // On the link text: `[` is column 4.
    app.tabs[app.active].editor.place_caret(LineCol::new(0, 6));
    app.dispatch(Command::OpenLinkAtCaret);
    assert_eq!(active_path(&app), Some(root.join("docs/guide.md")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn open_link_at_caret_takes_a_caret_just_past_the_link() {
    // The caret a typed link leaves behind: right after its `)`, column 26.
    let (mut app, root) =
        workspace_app("source-link-caret-end", b"see [guide](docs/guide.md) now\n");
    app.tabs[app.active].editor.place_caret(LineCol::new(0, 26));
    assert_eq!(app.source_link_at_caret().as_deref(), Some("docs/guide.md"));
    // One further, on the space, nothing touches the caret.
    app.tabs[app.active].editor.place_caret(LineCol::new(0, 27));
    assert_eq!(app.source_link_at_caret(), None);
    // A caret at the very end of a line that ends in a URL touches it.
    let (mut app, other) = workspace_app("source-link-caret-eol", b"see https://example.com\n");
    app.tabs[app.active].editor.place_caret(LineCol::new(0, 23));
    assert_eq!(
        app.source_link_at_caret().as_deref(),
        Some("https://example.com")
    );
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(other);
}

#[test]
fn open_link_at_caret_off_a_link_says_so_and_stays() {
    let (mut app, root) = workspace_app(
        "source-link-caret-none",
        b"see [guide](docs/guide.md) now\n",
    );
    app.tabs[app.active].editor.place_caret(LineCol::new(0, 1));
    app.dispatch(Command::OpenLinkAtCaret);
    assert!(
        app.notifications
            .active()
            .iter()
            .any(|note| note.title.starts_with("open link: no link at the caret"))
    );
    assert_eq!(active_path(&app), Some(root.join("README.md")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn open_link_at_caret_on_an_outside_link_asks_first() {
    let (mut app, root) = workspace_app("source-link-caret-outside", b"[out](../outside.md)\n");
    app.tabs[app.active].editor.place_caret(LineCol::new(0, 2));
    app.dispatch(Command::OpenLinkAtCaret);
    assert!(
        app.confirm.is_some(),
        "a link out of the workspace asks first"
    );
    assert_eq!(active_path(&app), Some(root.join("README.md")));
    let _ = std::fs::remove_dir_all(root);
}

/// The cells of `buffer` whose symbol carries an OSC 8 hyperlink whose URI
/// contains `needle`.
fn linked_cells(buffer: &ratatui::buffer::Buffer, needle: &str) -> usize {
    buffer
        .content()
        .iter()
        .filter(|cell| {
            let symbol = cell.symbol();
            symbol.starts_with("\u{1b}]8;")
                && symbol
                    .split('\u{1b}')
                    .nth(1)
                    .is_some_and(|opener| opener.contains(needle))
        })
        .count()
}

#[test]
fn source_links_paint_as_osc8_only_where_they_open_without_asking() {
    let url = "https://example.com/a";
    let (mut app, root) = workspace_app(
        "source-link-osc8",
        format!("see {url} and [out](../outside.md) and [g](docs/guide.md)\n").as_bytes(),
    );
    let buffer = frame(&mut app, 100, 12);

    assert_eq!(
        linked_cells(&buffer, url),
        url.len(),
        "one cell per character"
    );
    // `[g](docs/guide.md)`, a workspace file, links as a `file:` URI.
    assert_eq!(linked_cells(&buffer, "docs/guide.md"), 18);
    assert_eq!(
        linked_cells(&buffer, "outside"),
        0,
        "a link out of the workspace keeps its confirmation"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_wrapped_source_link_links_every_row_it_occupies() {
    // Markdown wraps by default, so in a 50-column window this URL takes
    // more than one row.
    let url = "https://example.com/a/rather/long/path/that/wraps/across/rows";
    let (mut app, root) = workspace_app("source-link-osc8-wrap", format!("{url}\n").as_bytes());
    let buffer = frame(&mut app, 50, 12);

    let rows = (0..buffer.area.height)
        .filter(|&y| {
            (0..buffer.area.width).any(|x| buffer[(x, y)].symbol().starts_with("\u{1b}]8;"))
        })
        .count();
    assert!(rows >= 2, "the link spans {rows} rows");
    assert_eq!(linked_cells(&buffer, url), url.len());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_source_link_under_a_collapsed_fold_is_not_painted() {
    let (mut app, root) = workspace_app(
        "source-link-osc8-fold",
        b"# Title\nsee https://example.com/hidden\nmore\nhttps://example.com/shown\n",
    );
    let idx = app.active;
    if let TabKind::Code { folds, folded, .. } = &mut app.tabs[idx].kind {
        *folds = FoldRegions::from_sorted_regions(vec![FoldRegion::new(0, 2)]);
        folded.insert(0);
    }
    let buffer = frame(&mut app, 100, 12);

    assert_eq!(linked_cells(&buffer, "hidden"), 0);
    assert_eq!(
        linked_cells(&buffer, "shown"),
        "https://example.com/shown".len(),
        "the line after the fold still links, on the row the fold freed"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_source_link_scrolled_half_out_of_view_links_only_its_drawn_cells() {
    // Code does not wrap, so the line scrolls horizontally.
    let url = "https://example.com/scrolled/out/of/view";
    let (mut app, root) = workspace_app("source-link-osc8-scroll", b"\n");
    write_file(
        &root,
        "main.rs",
        format!("// 3456789 {url} tail\n").as_bytes(),
    );
    app.open_path(&root.join("main.rs"));
    fill_reserved_tabs(&mut app);
    app.sidebar_visible = false;
    let _ = frame(&mut app, 40, 8);
    // Scrolled right past the URL's start at column 11, in a pane too narrow
    // for its end: it is cut off on both sides.
    let area = app.last_frame.editor_rect();
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: area.x.saturating_add(5),
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    for _ in 0..4 {
        app.handle_mouse(wheel);
    }
    let buffer = frame(&mut app, 40, 8);

    let editor = &app.tabs[app.active].editor;
    let (left, width) = (
        editor.scroll_col() as usize,
        editor.content_width() as usize,
    );
    assert!(left > 11, "scrolled to {left}");
    assert!(
        left + width < 11 + url.len(),
        "{width} columns show the URL's end"
    );
    assert_eq!(linked_cells(&buffer, url), width);
    let _ = std::fs::remove_dir_all(root);
}

/// Open Link at Caret follows a language server's link the caret is on
/// before reading the source, as Ctrl/Cmd+click does under the pointer.
#[test]
fn open_link_at_caret_prefers_a_server_link() {
    use karet_core::DocumentLink;
    use karet_core::Range;

    const DOC: DocumentId = DocumentId(9);
    let (mut app, root) = workspace_app(
        "source-link-caret-server",
        b"see [guide](docs/guide.md) now\n",
    );
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DOC);
    }
    write_file(&root, "other.md", b"other");
    let target = url::Url::from_file_path(root.join("other.md"))
        .map(String::from)
        .unwrap_or_default();
    let version = app.buffer_version(DOC).unwrap_or_default();
    let link = DocumentLink {
        range: Range {
            start: LineCol::new(0, 4),
            end: LineCol::new(0, 26),
        },
        target: Some(target),
        tooltip: None,
        payload: None,
    };
    app.links_colors.links.insert(DOC, (version, vec![link]));

    app.tabs[app.active].editor.place_caret(LineCol::new(0, 6));
    app.dispatch(Command::OpenLinkAtCaret);
    fill_reserved_tabs(&mut app);
    let opened = std::fs::canonicalize(root.join("other.md")).ok();
    assert_eq!(active_path(&app), opened);
    let _ = std::fs::remove_dir_all(root);
}
