//! Pointer text selection on the read-only surfaces: dragging highlights rows,
//! and `Ctrl+C` copies the content without the chrome painted around it.

use karet_widgets::RowPos;
use karet_widgets::RowSelection;

use super::support::*;
use crate::app::*;

/// An app showing a diff tab for a one-line change, already rendered once so the
/// selectable regions of the last frame are recorded.
fn diff_app(old: &str, new: &str) -> App {
    let dir = test_dir("surface-select");
    let changed = ChangeSummary {
        path: PathBuf::from("a.txt"),
        old_path: None,
        status: StatusKind::Modified,
        is_binary: false,
        added: 1,
        removed: 1,
    };
    let mut app = App::new(dir, Vec::new(), vec![changed], false);
    app.backend = Some(Arc::new(RecordingBackend::new()));
    app.sidebar_panel = SidebarPanel::SourceControl;
    app.focus = Focus::Sidebar;
    app.dispatch(Command::SidebarActivate);
    app.on_backend_event(
        None,
        SessionEvent::ChangePrepared {
            path: PathBuf::from("a.txt"),
            staged: false,
            result: Ok(Box::new(prepared_from_texts(
                "a.txt",
                StatusKind::Modified,
                old,
                new,
            ))),
        },
    );
    app
}

/// Press, drag and release across the surface under the pointer.
fn drag(app: &mut App, from: (u16, u16), to: (u16, u16)) {
    app.handle_mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), from));
    app.handle_mouse(mouse_at(MouseEventKind::Drag(MouseButton::Left), to));
    app.handle_mouse(mouse_at(MouseEventKind::Up(MouseButton::Left), to));
}

fn mouse_at(kind: MouseEventKind, at: (u16, u16)) -> MouseEvent {
    MouseEvent {
        kind,
        column: at.0,
        row: at.1,
        modifiers: KeyModifiers::NONE,
    }
}

/// The region recorded for `surface` in the focused pane.
fn region(app: &App, surface: SelectSurface) -> SelectRegion {
    let recorded = app.select_region(surface);
    assert!(recorded.is_some(), "{surface:?} should have been recorded");
    recorded.unwrap_or(SelectRegion {
        surface,
        area: Rect::default(),
        first_row: 0,
        hscroll: 0,
    })
}

#[test]
fn dragging_a_unified_diff_selects_rows_without_their_gutters() {
    let mut app = diff_app("alpha\nbravo\ncharlie\n", "alpha\nBRAVO\ncharlie\n");
    screen(&mut app, 80, 24);
    let region = region(&app, SelectSurface::Unified);

    // Find the row painting the added line, and select its text end to end.
    let added = (0..region.area.height)
        .map(|offset| region.first_row + usize::from(offset))
        .find(|row| {
            app.surface_row(&region, *row)
                .is_some_and(|painted| painted.text == "BRAVO")
        });
    assert!(added.is_some(), "the added line should be a selectable row");
    let Some(added) = added else { return };
    let painted = app.surface_row(&region, added);
    assert!(painted.is_some());
    let Some(painted) = painted else { return };

    let y = region.area.y + u16::try_from(added - region.first_row).unwrap_or_default();
    let start = region.area.x + painted.content_x;
    drag(&mut app, (start, y), (start + 5, y));

    assert_eq!(
        app.surface_selection_text().as_deref(),
        Some("BRAVO"),
        "the gutter and the `+` marker are chrome, not content"
    );
    app.dispatch(Command::Copy);
    assert_eq!(app.status.as_deref(), Some("copied selection"));
}

#[test]
fn dragging_down_a_unified_diff_spans_several_rows() {
    let mut app = diff_app("one\ntwo\nthree\n", "ONE\nTWO\nTHREE\n");
    screen(&mut app, 80, 24);
    let region = region(&app, SelectSurface::Unified);

    // Rows the diff paints as content, in order, with their screen rows.
    let rows: Vec<(u16, String)> = (0..region.area.height)
        .filter_map(|offset| {
            let row = region.first_row + usize::from(offset);
            let painted = app.surface_row(&region, row)?;
            Some((region.area.y + offset, painted.text))
        })
        .collect();
    assert!(rows.len() >= 2, "a multi-line change paints several rows");

    let content_x = region.area.x
        + app
            .surface_row(&region, region.first_row + 1)
            .map_or(0, |painted| painted.content_x);
    drag(
        &mut app,
        (content_x, rows[0].0),
        (content_x + 40, rows[1].0),
    );

    let text = app.surface_selection_text().unwrap_or_default();
    assert!(
        text.contains(&rows[0].1) && text.contains(&rows[1].1),
        "a downward drag covers both rows: {text:?}"
    );
    assert!(text.contains('\n'), "rows are joined by newlines: {text:?}");
}

#[test]
fn a_side_by_side_drag_never_bleeds_into_the_other_column() {
    let mut app = diff_app("before\n", "after\n");
    if let Some(Tab {
        kind: TabKind::Diff { view, .. },
        ..
    }) = app.tabs.get_mut(app.active)
    {
        *view = ViewMode::SideBySide;
    }
    screen(&mut app, 80, 24);

    let old = region(&app, SelectSurface::OldColumn);
    let new = region(&app, SelectSurface::NewColumn);
    assert!(
        old.area.right() <= new.area.x,
        "the old column sits left of the new one"
    );

    // Select the old side, dragging well past its right edge into the new one.
    let row = (0..old.area.height)
        .map(|offset| old.first_row + usize::from(offset))
        .find(|row| app.surface_row(&old, *row).is_some());
    assert!(row.is_some());
    let Some(row) = row else { return };
    let painted = app.surface_row(&old, row);
    let Some(painted) = painted else { return };
    let y = old.area.y + u16::try_from(row - old.first_row).unwrap_or_default();

    drag(
        &mut app,
        (old.area.x + painted.content_x, y),
        (new.area.right(), y),
    );

    assert_eq!(
        app.surface_selection_text().as_deref(),
        Some("before"),
        "dragging past the divider selects the old side only"
    );
}

#[test]
fn clicking_a_diff_without_dragging_selects_nothing_to_copy() {
    let mut app = diff_app("alpha\n", "beta\n");
    screen(&mut app, 80, 24);
    let region = region(&app, SelectSurface::Unified);
    let point = (region.area.x + 10, region.area.y + 1);

    app.handle_mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), point));
    app.handle_mouse(mouse_at(MouseEventKind::Up(MouseButton::Left), point));

    assert!(
        app.surface_selection.is_some(),
        "a caret-less anchor is set"
    );
    assert_eq!(
        app.surface_selection_text(),
        None,
        "an empty selection copies nothing"
    );
    // The drag capture is released on button-up.
    assert!(app.surface_selecting.is_none());
}

#[test]
fn the_selected_run_is_painted_with_the_selection_background() {
    let mut app = diff_app("alpha\nbravo\n", "alpha\nBRAVO\n");
    screen(&mut app, 80, 24);
    let region = region(&app, SelectSurface::Unified);

    let added = (0..region.area.height)
        .map(|offset| region.first_row + usize::from(offset))
        .find(|row| {
            app.surface_row(&region, *row)
                .is_some_and(|painted| painted.text == "BRAVO")
        });
    assert!(added.is_some());
    let Some(added) = added else { return };
    let Some(painted) = app.surface_row(&region, added) else {
        return;
    };
    let y = region.area.y + u16::try_from(added - region.first_row).unwrap_or_default();
    let start = region.area.x + painted.content_x;

    // Select "BRA" — three cells of a five-character row.
    drag(&mut app, (start, y), (start + 3, y));
    let buffer = frame(&mut app, 80, 24);

    let selection = app.theme.role(ThemeRole::Selection).to_ratatui();
    let bg = |x: u16| buffer.cell((x, y)).map(|cell| cell.bg);
    for x in start..start + 3 {
        assert_eq!(bg(x), Some(selection), "column {x} should be highlighted");
    }
    assert_ne!(
        bg(start + 3),
        Some(selection),
        "the unselected remainder of the row keeps its own background"
    );
    assert_ne!(
        bg(region.area.x),
        Some(selection),
        "the gutter is never highlighted"
    );
}

#[test]
fn dragging_a_hex_dump_copies_its_bytes_without_the_offset_column() {
    let mut app = app();
    let bytes: Vec<u8> = (0u8..48).collect();
    app.tabs = vec![Tab::new(
        "blob.bin",
        TabKind::Hex {
            path: PathBuf::from("blob.bin"),
            bytes: bytes.clone(),
            scroll: 0,
        },
    )];
    app.active = 0;
    screen(&mut app, 100, 24);
    let region = region(&app, SelectSurface::Hex);

    let Some(first) = app.surface_row(&region, 0) else {
        return;
    };
    let start = region.area.x + first.content_x;
    // Select the first two byte columns of the first row.
    drag(&mut app, (start, region.area.y), (start + 6, region.area.y));

    assert_eq!(
        app.surface_selection_text().as_deref(),
        Some("00 01 "),
        "the file-offset column is chrome, so it is never copied"
    );
}

#[test]
fn a_hex_selection_spanning_rows_copies_each_row_in_full() {
    let mut app = app();
    let bytes: Vec<u8> = (0u8..48).collect();
    app.tabs = vec![Tab::new(
        "blob.bin",
        TabKind::Hex {
            path: PathBuf::from("blob.bin"),
            bytes,
            scroll: 0,
        },
    )];
    app.active = 0;
    screen(&mut app, 100, 24);
    let region = region(&app, SelectSurface::Hex);
    let Some(first) = app.surface_row(&region, 0) else {
        return;
    };
    let start = region.area.x + first.content_x;

    drag(
        &mut app,
        (start, region.area.y),
        (start + 200, region.area.y + 1),
    );

    let text = app.surface_selection_text().unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "two rows selected: {text:?}");
    assert!(lines[0].starts_with("00 01 02"), "{:?}", lines[0]);
    assert!(lines[1].starts_with("10 11 12"), "{:?}", lines[1]);
    assert!(lines[0].ends_with('|'), "the ASCII column comes along too");
}

#[test]
fn dragging_a_markdown_preview_copies_the_text_it_renders() {
    let mut app = app();
    app.tabs = vec![Tab::document_preview(
        PathBuf::from("notes.md"),
        "# Title\n\nA paragraph of prose.\n",
    )];
    app.active = 0;
    screen(&mut app, 80, 24);
    let region = region(&app, SelectSurface::MarkdownPreview);

    // Find the rendered row carrying the paragraph.
    let prose = (0..region.area.height)
        .map(|offset| region.first_row + usize::from(offset))
        .find(|row| {
            app.surface_row(&region, *row)
                .is_some_and(|painted| painted.text.contains("paragraph"))
        });
    assert!(prose.is_some(), "the paragraph should be a selectable row");
    let Some(prose) = prose else { return };
    let Some(painted) = app.surface_row(&region, prose) else {
        return;
    };

    let y = region.area.y + u16::try_from(prose - region.first_row).unwrap_or_default();
    let width = u16::try_from(painted.text.len()).unwrap_or(u16::MAX);
    drag(&mut app, (region.area.x, y), (region.area.x + width, y));

    assert_eq!(
        app.surface_selection_text().as_deref(),
        Some(painted.text.as_str()),
        "the whole rendered row comes across"
    );
}

#[test]
fn a_markdown_selection_carries_the_rendered_decorations() {
    let mut app = app();
    app.tabs = vec![Tab::document_preview(
        PathBuf::from("notes.md"),
        "- first item\n- second item\n",
    )];
    app.active = 0;
    screen(&mut app, 80, 24);
    let region = region(&app, SelectSurface::MarkdownPreview);

    let bullet = (0..region.area.height)
        .map(|offset| region.first_row + usize::from(offset))
        .find(|row| {
            app.surface_row(&region, *row)
                .is_some_and(|painted| painted.text.contains("first item"))
        });
    assert!(bullet.is_some());
    let Some(bullet) = bullet else { return };
    let Some(painted) = app.surface_row(&region, bullet) else {
        return;
    };
    // A preview row's text is what the reader sees, list marker included — the
    // markdown source is not recoverable per row.
    assert!(
        !painted.text.starts_with("- "),
        "the raw source marker is not what the preview paints: {:?}",
        painted.text
    );
    assert!(painted.text.contains("first item"));
}

/// The commit-card region recorded in the focused pane, whatever prefix the
/// current layout gave it.
fn cards_region(app: &App) -> Option<SelectRegion> {
    app.pane_frames
        .first()?
        .select_regions
        .iter()
        .find(|region| matches!(region.surface, SelectSurface::CommitCards { .. }))
        .copied()
}

/// The first card row carrying content, as `(screen row, content)`.
fn first_card_row(app: &App, region: &SelectRegion) -> Option<(u16, SurfaceRow)> {
    (0..region.area.height).find_map(|offset| {
        let row = region.first_row + usize::from(offset);
        let painted = app.surface_row(region, row)?;
        (!painted.text.is_empty()).then(|| (region.area.y + offset, painted))
    })
}

#[test]
fn dragging_a_commit_file_card_copies_the_diff_without_its_rail() {
    let mut app = responsive_commit_app();
    // Wide enough for the rail-and-diff layout.
    screen(&mut app, 120, 24);
    let region = cards_region(&app);
    assert!(region.is_some(), "the wide layout records a card region");
    let Some(region) = region else { return };
    assert!(
        matches!(
            region.surface,
            SelectSurface::CommitCards { prefix_rows: 0 }
        ),
        "the wide layout keeps its file index in a separate rail"
    );

    let found = first_card_row(&app, &region);
    assert!(found.is_some(), "a card body row should be selectable");
    let Some((y, painted)) = found else { return };

    let start = region.area.x + painted.content_x;
    let width = u16::try_from(painted.text.len()).unwrap_or(u16::MAX);
    drag(&mut app, (start, y), (start + width, y));

    assert_eq!(
        app.surface_selection_text().as_deref(),
        Some(painted.text.as_str()),
        "the card rail and the diff gutter are both chrome"
    );
}

#[test]
fn the_stacked_layout_records_its_file_index_as_prefix_rows() {
    let mut app = responsive_commit_app();
    // Below the 104-column breakpoint the file index stacks above the cards.
    screen(&mut app, 100, 24);
    let region = cards_region(&app);
    assert!(region.is_some(), "the stacked layout records a card region");
    let Some(region) = region else { return };

    let SelectSurface::CommitCards { prefix_rows } = region.surface else {
        return;
    };
    assert!(
        prefix_rows > 0,
        "the summary and per-file index rows precede the first card"
    );
    // Those prefix rows are chrome: they carry no selectable content.
    for row in region.first_row..region.first_row + usize::from(prefix_rows) {
        assert_eq!(
            app.surface_row(&region, row),
            None,
            "prefix row {row} is the file index, not diff content"
        );
    }
}

#[test]
fn a_commit_selection_skips_the_chrome_between_two_cards() {
    let mut app = responsive_commit_app();
    screen(&mut app, 120, 40);
    let Some(region) = cards_region(&app) else {
        return;
    };

    // Every row the cards paint, tagged with whether it carries content.
    let rows: Vec<(usize, Option<String>)> = (0..region.area.height)
        .map(|offset| {
            let row = region.first_row + usize::from(offset);
            (row, app.surface_row(&region, row).map(|p| p.text))
        })
        .collect();
    let content: Vec<usize> = rows
        .iter()
        .filter(|(_, text)| text.is_some())
        .map(|(row, _)| *row)
        .collect();
    assert!(content.len() >= 2, "several body rows are visible");

    // Selecting across the whole visible span copies the body rows and leaves
    // blank lines where the card borders and separators were.
    let Some(first) = rows.first().map(|(row, _)| *row) else {
        return;
    };
    let Some(last) = rows.last().map(|(row, _)| *row) else {
        return;
    };
    let mut selection = RowSelection::new(RowPos::new(first, 0));
    selection.extend_to(RowPos::new(last, 0));
    app.surface_selection = Some(SurfaceSelection {
        surface: region.surface,
        selection,
    });

    let text = app.surface_selection_text().unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), last - first, "one line per covered row");
    assert!(
        lines.iter().any(|line| !line.is_empty()),
        "body rows carry their diff text"
    );
    assert!(
        lines.iter().any(|line| line.is_empty()),
        "card headers, footers and separators come across as blank lines"
    );
}

#[test]
fn the_find_bar_supports_keyboard_and_mouse_selection() {
    let mut app = app();
    app.sidebar_visible = false;
    app.focus = Focus::Editor;
    app.push_tab(text_tab("t.rs", "alpha bravo charlie"));
    app.dispatch(Command::OpenFind);
    for c in "bravo".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(
        app.active_find().map(|f| f.query.clone()),
        Some("bravo".into())
    );

    // Shift+Left extends a selection back over the last two characters.
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.modal_selection_text().as_deref(), Some("vo"));

    // Ctrl+C copies the field, not the editor beneath it.
    app.dispatch(Command::Copy);
    assert_eq!(app.status.as_deref(), Some("copied selection"));

    // Ctrl+A selects the whole field; Ctrl+X takes it away.
    app.dispatch(Command::EditorSelectAll);
    assert_eq!(app.modal_selection_text().as_deref(), Some("bravo"));
    app.dispatch(Command::Cut);
    assert_eq!(
        app.active_find().map(|f| f.query.clone()),
        Some(String::new())
    );
}

#[test]
fn dragging_the_find_field_selects_the_text_under_the_pointer() {
    let mut app = app();
    app.sidebar_visible = false;
    app.focus = Focus::Editor;
    app.push_tab(text_tab("t.rs", "alpha"));
    app.dispatch(Command::OpenFind);
    for c in "alphabet".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    screen(&mut app, 80, 24);

    let field = app.find_rects.query;
    assert!(field.width > 0, "the find bar records its query field");
    drag(&mut app, (field.x + 5, field.y), (field.x + 8, field.y));

    assert_eq!(app.modal_selection_text().as_deref(), Some("bet"));
    assert!(
        app.text_field_drag.is_none(),
        "the drag is released on button-up"
    );
}

#[test]
fn clicking_the_replace_row_moves_the_edited_field() {
    let mut app = app();
    app.sidebar_visible = false;
    app.focus = Focus::Editor;
    app.push_tab(text_tab("t.rs", "alpha"));
    app.dispatch(Command::OpenFind);
    app.dispatch(Command::FindToggleReplace);
    if let Some(find) = app.active_find_mut() {
        find.replace = "replacement".to_string();
    }
    screen(&mut app, 80, 24);

    let replace = app.find_rects.replace;
    assert!(replace.is_some(), "the replace row records its field");
    let Some(replace) = replace else { return };

    drag(&mut app, (replace.x, replace.y), (replace.x + 7, replace.y));
    assert_eq!(
        app.active_find().map(|f| f.field),
        Some(crate::tab::SearchField::Replace),
        "clicking the replace row starts editing it"
    );
    assert_eq!(app.modal_selection_text().as_deref(), Some("replace"));
}

#[test]
fn dragging_the_explorer_rename_field_selects_within_the_name() {
    let dir = test_dir("explorer-rename-select");
    write_file(&dir, "readme.md", b"hi\n");
    let mut app = App::new(dir, Vec::new(), Vec::new(), false);
    app.sidebar_panel = SidebarPanel::Explorer;
    app.focus = Focus::Sidebar;
    screen(&mut app, 80, 24);
    app.dispatch(Command::ExplorerRename);
    screen(&mut app, 80, 24);

    let rect = app.explorer.edit_rect();
    assert!(rect.is_some(), "the rename row reports its field");
    let Some(rect) = rect else { return };

    // Drag across the extension.
    drag(&mut app, (rect.x + 6, rect.y), (rect.x + 9, rect.y));
    assert_eq!(app.modal_selection_text().as_deref(), Some(".md"));

    // Ctrl+C reaches the field rather than the editor behind the sidebar.
    app.dispatch(Command::Copy);
    assert_eq!(app.status.as_deref(), Some("copied selection"));
}
