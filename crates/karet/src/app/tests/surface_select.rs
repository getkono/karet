//! Pointer text selection on the read-only surfaces: dragging highlights rows,
//! and `Ctrl+C` copies the content without the chrome painted around it.

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
            app.surface_row(SelectSurface::Unified, *row)
                .is_some_and(|painted| painted.text == "BRAVO")
        });
    assert!(added.is_some(), "the added line should be a selectable row");
    let Some(added) = added else { return };
    let painted = app.surface_row(SelectSurface::Unified, added);
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
            let painted = app.surface_row(SelectSurface::Unified, row)?;
            Some((region.area.y + offset, painted.text))
        })
        .collect();
    assert!(rows.len() >= 2, "a multi-line change paints several rows");

    let content_x = region.area.x
        + app
            .surface_row(SelectSurface::Unified, region.first_row + 1)
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
        .find(|row| app.surface_row(SelectSurface::OldColumn, *row).is_some());
    assert!(row.is_some());
    let Some(row) = row else { return };
    let painted = app.surface_row(SelectSurface::OldColumn, row);
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
            app.surface_row(SelectSurface::Unified, *row)
                .is_some_and(|painted| painted.text == "BRAVO")
        });
    assert!(added.is_some());
    let Some(added) = added else { return };
    let Some(painted) = app.surface_row(SelectSurface::Unified, added) else {
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
