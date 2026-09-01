//! The commit view's changed-file index: its directory tree, its folds, and the
//! rail's own scroll (issue #273).

use super::support::*;
use crate::app::*;

/// A commit across three directories, deep enough that the wide rail overflows a
/// short terminal — the shape the flat list was unreadable at.
fn deep_commit_app() -> App {
    let mut paths = Vec::new();
    for dir in ["crates/karet/src/app", "crates/karet/src/ui", "docs"] {
        for name in ["one.rs", "two.rs", "three.rs", "four.rs"] {
            paths.push(format!("{dir}/{name}"));
        }
    }
    let mut app = app();
    app.sidebar_visible = false;
    app.focus = Focus::Editor;
    app.push_tab(Tab::commit(
        Box::new(commit_detail(&"a".repeat(40), "a wide commit")),
        CommitFiles::ready(
            paths
                .iter()
                .map(|path| FileView::new(prepared_change(path, StatusKind::Modified)))
                .collect(),
        ),
    ));
    app
}

fn view(app: &App) -> &crate::tab::CommitViewState {
    match &app.tabs[app.active].kind {
        TabKind::Commit { view, .. } => view,
        _ => panic!("expected commit tab"),
    }
}

/// A press routed the way a real one is — through `handle_mouse`, so the surface
/// selection and every other claimant get their turn ahead of the editor.
fn click(app: &mut App, x: u16, y: u16) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

fn wheel(app: &mut App, kind: MouseEventKind, x: u16, y: u16) {
    app.handle_mouse(MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

/// The shared prefix is stated once, on a directory row, instead of being repeated
/// (and left-truncated away) on every file row. This is the readability the issue
/// asked for.
#[test]
fn the_rail_states_a_shared_directory_once() {
    let mut app = deep_commit_app();
    let painted = screen(&mut app, 120, 24);
    let rail = app.pane_frames[0].commit_rail_rect;
    let column = painted
        .iter()
        .skip(usize::from(rail.y))
        .take(usize::from(rail.height))
        .map(|row| {
            row.chars()
                .skip(usize::from(rail.x))
                .take(usize::from(rail.width))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    // The chain is compacted onto one row, and the four files under it print their
    // names alone — the flat list repeated (and truncated) that prefix twelve times.
    assert!(column.contains("crates/karet/src"), "{column}");
    assert_eq!(
        column.matches("crates/karet/src").count(),
        1,
        "the prefix is stated once, not per file: {column}"
    );
    assert!(column.contains("one.rs"), "{column}");
}

/// The rail scrolls on its own: a wheel notch over it moves the file list and
/// leaves the diff exactly where it was. This is the de-sync #273 asked for.
#[test]
fn the_rail_wheel_scrolls_the_rail_and_not_the_diff() {
    let mut app = deep_commit_app();
    let _ = screen(&mut app, 120, 12);
    let rail = app.pane_frames[0].commit_rail_rect;
    assert!(rail.height > 0, "the wide layout paints a rail");
    let diff_before = view(&app).scroll;

    wheel(&mut app, MouseEventKind::ScrollDown, rail.x + 1, rail.y + 1);
    let _ = screen(&mut app, 120, 12);
    assert!(view(&app).rail_scroll > 0, "the rail moved");
    assert_eq!(view(&app).scroll, diff_before, "the diff did not");

    wheel(&mut app, MouseEventKind::ScrollUp, rail.x + 1, rail.y + 1);
    let _ = screen(&mut app, 120, 12);
    assert_eq!(view(&app).rail_scroll, 0);
    assert_eq!(view(&app).scroll, diff_before);
}

/// A wheel notch outside the rail still scrolls the document, so the rail claims
/// only its own column.
#[test]
fn the_wheel_beside_the_rail_still_scrolls_the_diff() {
    let mut app = deep_commit_app();
    let _ = screen(&mut app, 120, 12);
    let rail = app.pane_frames[0].commit_rail_rect;
    wheel(
        &mut app,
        MouseEventKind::ScrollDown,
        rail.right() + 4,
        rail.y + 1,
    );
    assert!(view(&app).scroll > 0, "the diff moved");
    assert_eq!(view(&app).rail_scroll, 0, "the rail did not");
}

/// Clicking a directory row folds it, and its files leave the index; clicking again
/// brings them back.
#[test]
fn clicking_a_directory_row_folds_and_unfolds_it() {
    for width in [80, 120] {
        let mut app = deep_commit_app();
        let _ = screen(&mut app, width, 24);
        let files_before = app.pane_frames[0].commit_file_hits.len();
        let hit = app.pane_frames[0].commit_dir_hits[0].clone();
        assert!(files_before > 0, "at width {width}");

        click(&mut app, hit.rect.x + 1, hit.rect.y);
        let _ = screen(&mut app, width, 24);
        assert!(
            app.pane_frames[0].commit_file_hits.len() < files_before,
            "folding hides files at width {width}"
        );
        assert!(view(&app).collapsed_dirs.contains(&hit.path));

        // The row the fold left behind is the row that unfolds it.
        let again = app.pane_frames[0]
            .commit_dir_hits
            .iter()
            .find(|dir| dir.path == hit.path)
            .cloned()
            .unwrap_or_else(|| panic!("the folded directory keeps its row at width {width}"));
        click(&mut app, again.rect.x + 1, again.rect.y);
        let _ = screen(&mut app, width, 24);
        assert_eq!(
            app.pane_frames[0].commit_file_hits.len(),
            files_before,
            "unfolding restores them at width {width}"
        );
        assert!(view(&app).collapsed_dirs.is_empty());
    }
}

/// A file row still jumps the diff to that file's card — folding did not take the
/// index's original job away.
#[test]
fn clicking_a_file_row_still_jumps_the_diff() {
    let mut app = deep_commit_app();
    let _ = screen(&mut app, 120, 24);
    let hit = app.pane_frames[0].commit_file_hits[2];
    click(&mut app, hit.rect.x + 4, hit.rect.y);
    assert_eq!(view(&app).scroll, hit.scroll);
}

/// Folding a directory in the stacked layout shortens the index above the cards.
/// Without the prefix remap the anchors would slide up under a fixed offset and the
/// diff would jump; the card the reader was on has to stay put.
#[test]
fn folding_in_the_stacked_layout_keeps_the_diff_where_it_was() {
    /// The card the top of the screen is showing: the last anchor at or above it.
    fn card_on_screen(app: &App) -> Option<usize> {
        let view = view(app);
        view.file_anchors
            .iter()
            .rposition(|anchor| *anchor <= view.scroll)
    }

    let mut app = deep_commit_app();
    let _ = screen(&mut app, 80, 16);
    // Park the reader in the middle of the diff, clear of the index above it and of
    // the clamp at the end of the document.
    let anchor = view(&app).file_anchors[6];
    match &mut app.tabs[app.active].kind {
        TabKind::Commit { view, .. } => view.scroll = anchor,
        _ => panic!("expected commit tab"),
    }
    let _ = screen(&mut app, 80, 16);
    let before = view(&app).scroll;
    let rows_before = view(&app).prefix_rows;
    let card_before = card_on_screen(&app);
    assert_eq!(before, anchor, "the parked offset was not clamped away");

    // Scrolled this far in, the index is off the top of the screen and has no row to
    // click, so folding is the command's job.
    app.dispatch(Command::CommitFoldFileTree);
    let _ = screen(&mut app, 80, 16);

    let shrank = rows_before - view(&app).prefix_rows;
    assert!(shrank > 0, "folding removed index rows");
    assert_eq!(
        view(&app).scroll,
        before - shrank,
        "the offset followed the rows the fold removed"
    );
    assert_eq!(
        card_on_screen(&app),
        card_before,
        "the same card is still under the top of the screen"
    );
}

/// The rail follows the diff onto a file that had scrolled out of it, but a manual
/// scroll afterwards is left alone — the reveal fires on a change of file, not on
/// every frame.
#[test]
fn the_rail_reveals_a_new_active_file_without_fighting_a_manual_scroll() {
    let mut app = deep_commit_app();
    let _ = screen(&mut app, 120, 14);
    let start = view(&app).rail_scroll;

    for _ in 0..12 {
        app.dispatch(Command::NextChangedFile);
    }
    let _ = screen(&mut app, 120, 14);
    let revealed = view(&app).rail_scroll;
    assert!(
        revealed > start,
        "walking past the rail's bottom scrolled it ({start} -> {revealed})"
    );

    // Repainting on the same file must not move it again...
    let _ = screen(&mut app, 120, 14);
    assert_eq!(view(&app).rail_scroll, revealed);
    // ...nor may a repaint undo a scroll the user just made.
    let rail = app.pane_frames[0].commit_rail_rect;
    wheel(&mut app, MouseEventKind::ScrollUp, rail.x + 1, rail.y + 1);
    let manual = view(&app).rail_scroll;
    assert!(manual < revealed);
    let _ = screen(&mut app, 120, 14);
    assert_eq!(view(&app).rail_scroll, manual);
}

/// Fold-all reaches every directory in one pass, including the ones nested inside
/// another; unfold-all restores them.
#[test]
fn the_fold_all_command_folds_every_directory_and_unfold_all_restores_them() {
    let mut app = deep_commit_app();
    let _ = screen(&mut app, 120, 24);
    let files = app.pane_frames[0].commit_file_hits.len();
    assert!(files > 0);

    app.dispatch(Command::CommitFoldFileTree);
    let _ = screen(&mut app, 120, 24);
    assert!(
        app.pane_frames[0].commit_file_hits.is_empty(),
        "no file row survives a full fold"
    );
    assert!(
        view(&app)
            .collapsed_dirs
            .contains(Path::new("crates/karet/src/app")),
        "a directory nested inside another folds too, not just the outermost: {:?}",
        view(&app).collapsed_dirs
    );

    app.dispatch(Command::CommitUnfoldFileTree);
    let _ = screen(&mut app, 120, 24);
    assert!(view(&app).collapsed_dirs.is_empty());
    assert_eq!(app.pane_frames[0].commit_file_hits.len(), files);
}

/// A fold survives the layout breakpoint, and *both* layouts honour it: a fold no
/// layout read would keep the set intact and still list every file.
#[test]
fn a_fold_survives_a_resize_and_both_layouts_honour_it() {
    /// The changed files the index currently offers, by path.
    fn listed(app: &App) -> Vec<PathBuf> {
        let files = match &app.tabs[app.active].kind {
            TabKind::Commit { files, .. } => &files.files,
            _ => panic!("expected commit tab"),
        };
        app.pane_frames[0]
            .commit_file_hits
            .iter()
            .filter_map(|hit| files.get(hit.file).map(|f| f.change.path.clone()))
            .collect()
    }

    let mut app = deep_commit_app();
    let _ = screen(&mut app, 120, 24);
    let hit = app.pane_frames[0].commit_dir_hits[0].clone();
    assert_eq!(hit.path, PathBuf::from("crates/karet/src"));
    assert!(listed(&app).iter().any(|path| path.starts_with(&hit.path)));

    click(&mut app, hit.rect.x + 1, hit.rect.y);
    for width in [120, 80, 120] {
        let _ = screen(&mut app, width, 24);
        assert!(
            view(&app).collapsed_dirs.contains(&hit.path),
            "the fold survives the resize to {width}"
        );
        assert!(
            !listed(&app).iter().any(|path| path.starts_with(&hit.path)),
            "no file under the folded directory is listed at width {width}: {:?}",
            listed(&app)
        );
    }
}

/// The stacked index is painted inside the card region, which claims presses for
/// text selection. Its rows carry an action, so they have to win — and a real press
/// goes through `handle_mouse`, where that contest happens.
#[test]
fn the_stacked_index_rows_win_the_press_from_the_card_selection() {
    let mut app = deep_commit_app();
    let _ = screen(&mut app, 80, 24);

    // A directory row folds rather than starting a selection over it.
    let dir = app.pane_frames[0].commit_dir_hits[0].clone();
    click(&mut app, dir.rect.x + 1, dir.rect.y);
    assert!(view(&app).collapsed_dirs.contains(&dir.path));
    assert!(
        app.surface_selection.is_none(),
        "no text selection was started on the index"
    );

    // And a file row jumps the diff.
    let _ = screen(&mut app, 80, 24);
    let file = app.pane_frames[0].commit_file_hits[0];
    click(&mut app, file.rect.x + 6, file.rect.y);
    assert_eq!(view(&app).scroll, file.scroll);
    assert!(app.surface_selection.is_none());
}

/// A press on a diff row still starts a text selection — the index must not cost
/// the cards their selectability.
#[test]
fn a_press_on_a_diff_row_still_starts_a_selection() {
    let mut app = deep_commit_app();
    let _ = screen(&mut app, 80, 24);
    // Scroll into the first card, past the index entirely.
    let body = view(&app).file_anchors[0].saturating_add(2);
    match &mut app.tabs[app.active].kind {
        TabKind::Commit { view, .. } => view.scroll = body,
        _ => panic!("expected commit tab"),
    }
    let _ = screen(&mut app, 80, 24);
    assert!(
        app.pane_frames[0].commit_file_hits.is_empty()
            && app.pane_frames[0].commit_dir_hits.is_empty(),
        "the index has scrolled away, so only card rows are on screen"
    );
    let region = app.pane_frames[0].select_regions[0].area;
    click(&mut app, region.x + 8, region.y + 4);
    assert!(
        app.surface_selection.is_some(),
        "a card row is still selectable text"
    );
}

/// The boundary the prefix remap exists for: the reader parked on the blank row
/// that separates the index from the first card. That row belongs to the cards, not
/// the index, so it has to move with them.
#[test]
fn folding_remaps_the_row_immediately_below_the_index() {
    let mut app = deep_commit_app();
    let _ = screen(&mut app, 80, 16);
    // One row above the first card's anchor is the separator: the first row that is
    // no longer part of the index.
    let separator = view(&app).file_anchors[0].saturating_sub(1);
    match &mut app.tabs[app.active].kind {
        TabKind::Commit { view, .. } => view.scroll = separator,
        _ => panic!("expected commit tab"),
    }
    let _ = screen(&mut app, 80, 16);
    assert_eq!(
        view(&app).scroll,
        separator,
        "the park was not clamped away"
    );
    let rows_before = view(&app).prefix_rows;
    assert_eq!(
        separator, rows_before,
        "the separator sits exactly at the index's end"
    );

    app.dispatch(Command::CommitFoldFileTree);
    let _ = screen(&mut app, 80, 16);
    let shrank = rows_before - view(&app).prefix_rows;
    assert!(shrank > 0);
    assert_eq!(
        view(&app).scroll,
        separator - shrank,
        "the separator row moved up with the cards it belongs to"
    );
    assert_eq!(
        view(&app).scroll,
        view(&app).file_anchors[0].saturating_sub(1),
        "and it is still the separator above the first card"
    );
}

/// The rail's wheel writes the *active* tab's offset, so only the focused pane's
/// rail may claim a notch. Over another pane's rail the notch has to keep falling
/// through to the document scroll instead of landing on a view it cannot address.
#[test]
fn an_unfocused_panes_rail_does_not_swallow_the_wheel() {
    let mut app = deep_commit_app();
    app.dispatch(Command::SplitRight);
    assert_eq!(app.layout.pane_count(), 2);
    // Give the focused pane a code tab, leaving the commit view in the other one.
    app.push_tab(code_tab("a.rs"));
    let _ = screen(&mut app, 240, 24);
    let focused = app.focus_pane();
    let Some(other) = app
        .pane_frames
        .iter()
        .find(|frame| frame.pane != focused && frame.commit_rail_rect.height > 0)
        .map(|frame| frame.commit_rail_rect)
    else {
        panic!("the unfocused pane paints a rail");
    };

    let before = app.tabs[app.active].editor.scroll_line;
    wheel(
        &mut app,
        MouseEventKind::ScrollDown,
        other.x + 1,
        other.y + 1,
    );
    assert_ne!(
        app.tabs[app.active].editor.scroll_line, before,
        "the notch fell through to the focused document instead of being swallowed"
    );
}
