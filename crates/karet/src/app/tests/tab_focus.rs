//! Which tab — and which pane — takes focus when the one in front closes.

use super::support::*;
use crate::app::*;

/// Three code tabs in one pane, activated left to right, so the history is
/// `a.rs`, `b.rs`, `c.rs` with `c.rs` in front.
fn three_tabs() -> App {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    app.push_tab(code_tab("b.rs"));
    app.push_tab(code_tab("c.rs"));
    app
}

/// Four code tabs in one pane, so an activation order can be built that differs
/// from the positional answer at every step.
fn four_tabs() -> App {
    let mut app = three_tabs();
    app.push_tab(code_tab("d.rs"));
    app
}

fn active_title(app: &App) -> &str {
    app.tabs
        .get(app.active)
        .map_or("<none>", |tab| tab.title.as_str())
}

#[test]
fn closing_the_active_tab_focuses_the_last_active_one() {
    let mut app = three_tabs();
    app.select_tab(0);
    app.select_tab(2);

    app.close_tab_at(2);

    // Positionally the closed last tab would hand focus to `b.rs`, which the user
    // never opened; the history puts them back in the file they were working in.
    assert_eq!(active_title(&app), "a.rs");
}

#[test]
fn closing_walks_back_through_the_activation_history() {
    let mut app = four_tabs();
    for index in [1, 3, 0, 2] {
        app.select_tab(index);
    }

    // Each step's positional answer differs from the recency one: `d.rs`, then
    // `b.rs`, then `c.rs`.
    app.close_tab_at(app.active);
    assert_eq!(active_title(&app), "a.rs");

    app.close_tab_at(app.active);
    assert_eq!(active_title(&app), "d.rs");

    app.close_tab_at(app.active);
    assert_eq!(active_title(&app), "b.rs");
}

#[test]
fn a_reactivated_tab_moves_to_the_front_of_the_history() {
    let mut app = three_tabs();
    app.select_tab(1);
    app.select_tab(0);
    app.select_tab(2);

    app.close_tab_at(2);

    // `b.rs` was visited more recently than `a.rs` in absolute terms, but `a.rs`
    // was re-activated afterwards and so is the one to go back to.
    assert_eq!(active_title(&app), "a.rs");
}

#[test]
fn closing_a_background_tab_keeps_the_active_one() {
    let mut app = three_tabs();
    app.select_tab(1);

    app.close_tab_at(0);
    assert_eq!(active_title(&app), "b.rs");
    assert_eq!(app.active, 0);

    app.close_tab_at(1);
    assert_eq!(active_title(&app), "b.rs");
}

#[test]
fn focus_falls_back_to_the_neighbour_without_history() {
    // Tabs installed straight onto the field never record an activation, which is
    // also the state of a session that has only ever had tabs opened for it.
    let mut app = app();
    app.tabs = vec![code_tab("a.rs"), code_tab("b.rs"), code_tab("c.rs")];
    app.active = 1;
    assert!(app.view_history.is_empty());

    app.close_tab_at(1);
    assert_eq!(active_title(&app), "c.rs");

    app.close_tab_at(1);
    assert_eq!(active_title(&app), "a.rs");
}

#[test]
fn welcome_tabs_never_enter_the_activation_history() {
    let mut app = app();
    assert!(matches!(app.tabs[0].kind, TabKind::Welcome));

    app.select_tab(0);

    // Every welcome tab shares the unassigned `ViewId(0)`, so recording one would
    // let any pane's welcome tab answer for another's.
    assert!(app.view_history.is_empty());
}

#[test]
fn the_activation_history_holds_each_view_once() {
    let mut app = three_tabs();
    app.select_tab(0);
    app.select_tab(1);
    app.select_tab(0);

    let views: Vec<ViewId> = app.tabs.iter().map(|tab| tab.view).collect();
    assert_eq!(app.view_history, vec![views[2], views[1], views[0]]);
}

#[test]
fn closing_a_panes_last_tab_focuses_the_most_recently_active_pane() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    let first = app.focus_pane();
    app.dispatch(Command::SplitRight);
    let second = app.focus_pane();
    app.dispatch(Command::SplitRight);
    let third = app.focus_pane();
    assert_eq!(app.layout.pane_count(), 3);

    // Visit the far pane, then come back to the one about to collapse. The
    // positional neighbour of `third` is `second`, so recency and position differ.
    app.focus_pane_switch(first);
    app.focus_pane_switch(third);

    app.close_tab_at(app.active);

    assert_eq!(app.layout.pane_count(), 2);
    assert_eq!(app.focus_pane(), first);
    assert_ne!(app.focus_pane(), second);
    assert_eq!(app.focus, Focus::Editor);
}

#[test]
fn an_externally_deleted_active_file_falls_back_to_the_last_active_tab() {
    let mut app = three_tabs();
    app.select_tab(0);
    app.select_tab(2);

    app.close_tabs_under(&[PathBuf::from("c.rs")]);

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(active_title(&app), "a.rs");
}

#[test]
fn an_external_delete_keeps_a_surviving_active_tab_in_front() {
    let mut app = four_tabs();
    app.select_tab(2);

    // `a.rs` sat before the active tab, so clamping the old index left the
    // selection one tab past where the user actually was.
    app.close_tabs_under(&[PathBuf::from("a.rs")]);

    assert_eq!(app.tabs.len(), 3);
    assert_eq!(active_title(&app), "c.rs");
}

#[test]
fn dragging_the_active_tab_out_leaves_the_last_active_one_in_front() {
    let mut app = four_tabs();
    app.select_tab(0);
    app.select_tab(3);
    let origin = app.focus_pane();

    // Split `d.rs` out into its own pane; the origin needs a new tab in front.
    app.drop_tab_on(origin, DropZone::Right);
    assert_eq!(app.layout.pane_count(), 2);
    assert_eq!(active_title(&app), "d.rs");

    app.focus_pane_switch(origin);
    assert_eq!(active_title(&app), "a.rs");
}
