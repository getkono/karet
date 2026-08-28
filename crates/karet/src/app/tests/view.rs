use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use ratatui::style::Modifier;

use super::support::*;
use crate::app::*;
use crate::view::View;

/// A left click at `(column, row)`.
fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn selecting_a_view_moves_focus_onto_it() {
    let mut app = app();
    assert_eq!(app.view, View::Editor);
    assert_eq!(app.focus, Focus::Sidebar);

    app.dispatch(Command::SelectView(View::GitHub));
    assert_eq!(app.view, View::GitHub);
    // Load-bearing rather than merely tidy: focus follows the surface that now owns
    // the content area, so keys land on what the user is looking at.
    assert_eq!(app.focus, Focus::Editor);

    app.dispatch(Command::SelectView(View::Editor));
    assert_eq!(app.view, View::Editor);
}

// #211: the Agents view is the only one that hides the sidebar, so this has nothing
// to assert against until it exists.
// #[test]
// fn a_view_that_hides_the_sidebar_cannot_be_toggled_into_it() {
//     let mut app = app();
//     app.dispatch(Command::SelectView(View::Agents));
//     app.dispatch(Command::ToggleFocus);
//     assert_eq!(app.focus, Focus::Editor, "no sidebar to toggle into");
//
//     // The GitHub view keeps the sidebar, so Tab still reaches it there.
//     app.dispatch(Command::SelectView(View::GitHub));
//     app.dispatch(Command::ToggleFocus);
//     assert_eq!(app.focus, Focus::Sidebar);
// }

#[test]
fn the_view_decides_the_active_keymap_layers() {
    let mut app = app();
    app.dispatch(Command::SelectView(View::GitHub));
    assert_eq!(app.focus_target(), FocusTarget::Github);
    // The sidebar is not the content area, so it keeps its own layers whatever
    // view is showing.
    app.focus = Focus::Sidebar;
    assert_eq!(app.focus_target(), FocusTarget::Explorer);
}

#[test]
fn every_view_is_reachable_by_palette_title_and_slug() {
    for view in View::ALL {
        let command = Command::SelectView(view);
        assert_eq!(
            crate::command::resolve_named(command.label()),
            Ok(command),
            "{view:?} by title"
        );
        let slug = command.hint_verb().expect("a short slug");
        assert_eq!(
            crate::command::resolve_named(slug),
            Ok(command),
            "{view:?} by slug"
        );
    }
}

#[test]
fn the_startup_flag_selects_a_view() {
    let mut app = app();
    app.apply_startup_view(crate::cli::ViewChoice::Github);
    assert_eq!(app.view, View::GitHub);
}

#[test]
fn the_chrome_row_offers_every_view_and_marks_the_active_one() {
    let mut app = app();
    let rows = screen(&mut app, 100, 12);
    for view in View::ALL {
        assert!(rows[0].contains(view.title()), "{view:?} on the chrome row");
    }
    // The active view is the bold one; the others are not.
    let buffer = frame(&mut app, 100, 12);
    let bold = |x: u16| buffer[(x, 0)].style().add_modifier.contains(Modifier::BOLD);
    let hits = app.view_hits.clone();
    assert_eq!(hits.len(), View::ALL.len());
    for (start, _, view) in hits {
        assert_eq!(
            bold(start + 1),
            view == View::Editor,
            "{view:?} emphasis follows the active view"
        );
    }
}

#[test]
fn a_narrow_chrome_row_drops_every_label_at_once() {
    // All names or none: a row showing "Editor" beside two bare icons reads as a
    // rendering fault rather than as a deliberate compaction.
    let mut app = app();
    let rows = screen(&mut app, 16, 12);
    for view in View::ALL {
        assert!(
            !rows[0].contains(view.title()),
            "no labels at 16 columns: {:?}",
            rows[0]
        );
    }
    // Still one button per view, still clickable.
    assert_eq!(app.view_hits.len(), View::ALL.len());
    let (start, end, _) = app.view_hits[View::ALL.len() - 1];
    assert!(end <= 16, "the last button fits the row: {start}..{end}");
}

#[test]
fn clicking_the_chrome_row_switches_view() {
    let mut app = app();
    let _ = screen(&mut app, 100, 12);
    let (start, _, view) = app
        .view_hits
        .iter()
        .copied()
        .find(|&(_, _, view)| view == View::GitHub)
        .expect("a GitHub button");

    assert!(app.handle_view_chrome_mouse(click(start, 0)));
    assert_eq!(app.view, view);
    // The whole row belongs to the switcher: a click on its empty right-hand end
    // is consumed rather than falling through to the body below.
    assert!(app.handle_view_chrome_mouse(click(99, 0)));
    assert_eq!(app.view, view);
}

// #211: full-width layout and the placeholder body both come back with the Agents
// view; every view that ships today keeps the sidebar and draws a real surface.
// #[test]
// fn the_agents_view_takes_the_full_width_and_hides_the_sidebar() {
//     let mut app = app();
//     let _ = screen(&mut app, 100, 12);
//     assert!(
//         app.sidebar_rect.width > 0,
//         "the editor view keeps a sidebar"
//     );
//
//     app.dispatch(Command::SelectView(View::Agents));
//     let rows = screen(&mut app, 100, 12);
//     assert_eq!(app.sidebar_rect, Rect::default());
//     assert_eq!(app.main_rect.x, 0);
//     assert_eq!(app.main_rect.width, 100);
//     assert!(
//         rows.iter()
//             .any(|row| row.contains("Agents — not available yet")),
//         "the placeholder body names the view: {rows:?}"
//     );
// }

#[test]
fn the_github_hooks_do_not_fire_under_another_view() {
    // `github_key` and `github_mouse` run ahead of the keymap and read the GitHub
    // surface directly. That surface is off screen under the editor view, so a
    // keystroke or a click aimed at the editor must not drive it. (The same shape of
    // guard covers the Seam query box at `input.rs` and the unbound-printable
    // fallback into the active document.)
    let mut app = app();
    app.apply_github_availability(
        Some(super::support::repository()),
        super::github::anonymous_auth(),
    );
    app.focus = Focus::Editor;
    let section = |app: &App| {
        app.github
            .dashboard()
            .map(|dashboard| dashboard.section)
            .expect("the dashboard to be installed")
    };
    let before = section(&app);

    app.dispatch(Command::SelectView(View::Editor));
    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(section(&app), before, "the hidden surface is untouched");

    // Same for the click path, against a hit region the surface recorded for itself.
    if let Some(dashboard) = app.github.dashboard_mut() {
        dashboard.section_hits = vec![(
            crate::app::github::GithubSection::PullRequests,
            Rect::new(10, 2, 20, 1),
        )];
    }
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 12,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(section(&app), before, "a stale rect claims nothing");

    // Under its own view the same key reaches it.
    app.dispatch(Command::SelectView(View::GitHub));
    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_ne!(section(&app), before);
}

#[test]
fn a_view_over_the_editor_drops_the_panes_hit_regions() {
    // The pane shell is not drawn, so its rects from the last editor frame must not
    // keep claiming clicks — a press in the GitHub body would otherwise arm a text
    // drag in a document that is not on screen.
    let mut app = app();
    app.open_path(std::path::Path::new("Cargo.toml"));
    let _ = screen(&mut app, 100, 12);
    assert!(!app.pane_frames.is_empty());
    assert_ne!(app.editor_rect, Rect::default());

    app.dispatch(Command::SelectView(View::GitHub));
    let _ = screen(&mut app, 100, 12);
    assert!(app.pane_frames.is_empty());
    assert_eq!(app.editor_rect, Rect::default());
    assert!(app.pane_dividers.is_empty());
}

#[test]
fn the_status_bar_names_the_showing_view_and_drops_the_tab_strip() {
    let mut app = app();
    app.open_path(std::path::Path::new("Cargo.toml"));
    let editing = screen(&mut app, 100, 12);
    let status = editing.len() - 1;
    assert!(editing[status].contains("EDITOR"), "{:?}", editing[status]);

    app.dispatch(Command::SelectView(View::GitHub));
    let rows = screen(&mut app, 100, 12);
    assert!(rows[status].contains("GITHUB"), "{:?}", rows[status]);
    // The tab is still open behind the view, but its language describes a document
    // that is not on screen.
    assert!(
        !rows[status].contains("TOML"),
        "the tab-derived strip is dropped: {:?}",
        rows[status]
    );
}

#[test]
fn the_sidebar_survives_a_round_trip_through_another_view() {
    // The view gates *drawing* the sidebar, not the user's preference: coming back
    // to the editor must restore the sidebar rather than leave it collapsed.
    let mut app = app();
    app.dispatch(Command::SelectView(View::GitHub));
    let _ = screen(&mut app, 100, 12);
    app.dispatch(Command::SelectView(View::Editor));
    let _ = screen(&mut app, 100, 12);
    assert!(app.sidebar_visible);
    assert!(app.sidebar_rect.width > 0);
}
