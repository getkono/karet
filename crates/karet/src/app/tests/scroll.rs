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

/// A manager tab holding `count` synthetic servers, rendered once at `height` rows
/// so its hit regions and scrollbar track are live.
///
/// `count` has to overflow the painted window: a track is only recorded when the
/// extent overflows, so too few servers means there is no bar to grab.
fn language_server_app(count: usize, height: u16) -> App {
    let mut app = app();
    app.open_language_servers();
    let servers = (0..count)
        .map(|i| {
            language_server_status(
                LanguageServerId::new(format!("server-{i:02}")),
                "rust",
                true,
            )
        })
        .collect();
    app.show_language_server_status(None, servers);
    screen(&mut app, 100, height);
    app
}

/// A manager tab whose cards are *not* all the same height, which is the ordinary
/// case: only a managed server carries the full `Check updates / Restart / Uninstall`
/// strip, so an unmanaged one is a row shorter.
fn mixed_height_language_server_app(count: usize, height: u16) -> App {
    let mut app = app();
    app.open_language_servers();
    let servers = (0..count)
        .map(|i| {
            language_server_status(
                LanguageServerId::new(format!("server-{i:02}")),
                "rust",
                i == 0,
            )
        })
        .collect();
    app.show_language_server_status(None, servers);
    screen(&mut app, 100, height);
    app
}

/// Move the inventory's selection directly, as paging or a click would.
fn select_server(app: &mut App, index: usize) {
    match &mut app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view.selected = index,
        _ => panic!("expected the language-server manager"),
    }
}

/// The inventory's live scrollbar track.
fn inventory_track(app: &App) -> ScrollHit {
    app.scroll_hits
        .of(ScrollSurface::TabRows)
        .expect("an overflowing inventory should paint a bar")
}

/// The manager tab's selection and offset, both counted in servers.
fn inventory(app: &App) -> (usize, usize) {
    match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => (view.selected, view.offset),
        _ => panic!("expected the language-server manager"),
    }
}

#[test]
fn a_wheel_notch_over_the_language_server_inventory_moves_one_server() {
    // The inventory's offset is pinned to its selection by the render, so the wheel
    // has to move the selection. One server per notch, not the three lines the
    // editor moves: the cards are several rows tall.
    let mut app = language_server_app(16, 24);
    let rect = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view.table_rect,
        _ => panic!("expected the language-server manager"),
    };
    // Inside the table, clear of the track reserved on its right edge.
    let (x, y) = (rect.x + 2, rect.y + 2);

    wheel(&mut app, x, y, true);
    assert_eq!(
        inventory(&app).0,
        1,
        "a notch should move exactly one server"
    );

    // Once the selection leaves the painted window, the offset has to follow it.
    for _ in 0..10 {
        wheel(&mut app, x, y, true);
    }
    screen(&mut app, 100, 24);
    let (selected, offset) = inventory(&app);
    assert_eq!(selected, 11);
    assert!(offset > 0, "the window should have followed the selection");

    // And it clamps at the top rather than wrapping through zero. The offset only
    // catches up on the next paint, since that is what re-derives it.
    for _ in 0..40 {
        wheel(&mut app, x, y, false);
    }
    assert_eq!(
        inventory(&app).0,
        0,
        "the selection should clamp at the top"
    );
    screen(&mut app, 100, 24);
    assert_eq!(inventory(&app), (0, 0));
}

#[test]
fn dragging_the_language_server_thumb_scrolls_the_inventory_without_it_snapping_back() {
    // The guard the explorer already has, for the other member of the same family:
    // this view derives its offset from its selection during the render, so a
    // position written on its own is undone on the very next frame.
    let mut app = language_server_app(16, 24);
    let hit = inventory_track(&app);
    let track = hit.track.rect();
    let (start, _) = hit.track.thumb_span().unwrap_or_default();

    press(&mut app, track.x, track.y + start);
    assert!(
        app.scroll_drag.is_some(),
        "the track press must reach the bar, not be swallowed by the inventory's \
         blanket claim on its table rect"
    );
    drag(&mut app, track.x, track.bottom() - 1);

    let (selected, dragged) = inventory(&app);
    assert!(dragged > 0, "the drag should have scrolled the inventory");
    assert!(
        selected >= dragged,
        "the selection should have travelled into the window, which is why it holds"
    );

    // The next frame is where a naive offset write would be undone.
    screen(&mut app, 100, 24);
    assert_eq!(
        inventory(&app).1,
        dragged,
        "the inventory scrolled back on the next frame"
    );
}

#[test]
fn dragging_the_language_server_thumb_up_reaches_the_first_card_and_holds_it() {
    // Twenty-five rows, not twenty-four: at this height the cards do not tile the
    // pane exactly, so the bottom one is clipped. The clipped card is painted — it
    // shows there is more below — but it must not count toward the published
    // viewport, because the render's pin measures untruncated heights and demands a
    // full fit. Counting it advertises a window one card wider than the render will
    // keep, and the first server then becomes unreachable: the drag lands on it and
    // the next frame pushes it straight back.
    let mut app = language_server_app(16, 25);
    select_server(&mut app, 12);
    screen(&mut app, 100, 25);

    let hit = inventory_track(&app);
    let track = hit.track.rect();
    let (start, _) = hit.track.thumb_span().unwrap_or_default();
    press(&mut app, track.x, track.y + start);
    drag(&mut app, track.x, track.y);

    assert_eq!(
        inventory(&app).1,
        0,
        "dragging to the top should reach the first server"
    );
    screen(&mut app, 100, 25);
    assert_eq!(
        inventory(&app).1,
        0,
        "and the next frame must not push it back off the first server"
    );
}

#[test]
fn the_last_language_server_can_be_scrolled_fully_into_view() {
    // The other half of the same accounting: the extent's `max_position` is
    // `content - viewport`, so a viewport inflated by a clipped card would stop the
    // scroll one server short of the end.
    let mut app = language_server_app(16, 25);
    let extent = inventory_track(&app).track.extent();
    app.scroll_surface_to(
        ScrollSurface::TabRows,
        extent.max_position(),
        extent.viewport,
    );
    screen(&mut app, 100, 25);

    let rows = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view.row_hits.clone(),
        _ => panic!("expected the language-server manager"),
    };
    let (first, last) = match (rows.first(), rows.last()) {
        (Some(first), Some(last)) => (first, last),
        _ => panic!("the end of the scroll should still paint cards"),
    };
    assert_eq!(
        last.1,
        LanguageServerId::new("server-15"),
        "the end of the scroll should reach the last server"
    );
    // Reading the last card's height against the first only means anything while every
    // card is the same height, so make that dependence fail loudly rather than quietly
    // if the fixture ever gains a server with a different action strip.
    assert!(
        rows.iter().all(|row| row.0.height == first.0.height),
        "this test measures the last card against the first, which needs a uniform \
         fixture; use `mixed_height_language_server_app` for the variable case"
    );
    // A clipped card is painted short. At the end of the scroll the last card has to
    // be whole, or the user can never actually read it.
    assert_eq!(
        last.0.height, first.0.height,
        "the last server is still clipped at the end of the scroll"
    );
}

#[test]
fn dragging_a_mixed_height_inventory_to_the_top_lands_exactly() {
    // Cards are only uniform in a fixture. In practice a managed server carries a
    // taller action strip than an unmanaged one, and the `viewport` the extent
    // publishes was measured at the *old* offset — so it says nothing about how many
    // of the cards at the *new* position fit. Landing an out-of-window selection on
    // the window's bottom edge would therefore ask the pin for a fit nobody measured,
    // and it answers by nudging the offset past the position asked for. At these
    // heights that was permanent: the first server could never be reached from the
    // bar, however many times it was dragged.
    for height in [16_u16, 19, 25] {
        let mut app = mixed_height_language_server_app(16, height);
        select_server(&mut app, 12);
        screen(&mut app, 100, height);

        let hit = inventory_track(&app);
        let track = hit.track.rect();
        let (start, _) = hit.track.thumb_span().unwrap_or_default();
        press(&mut app, track.x, track.y + start);
        drag(&mut app, track.x, track.y);

        assert_eq!(
            inventory(&app).1,
            0,
            "one drag to the top should reach the first server at {height} rows"
        );
        screen(&mut app, 100, height);
        assert_eq!(
            inventory(&app).1,
            0,
            "and it must hold there across the next frame at {height} rows"
        );
    }
}

#[test]
fn a_pane_too_short_for_a_whole_card_still_scrolls_to_the_last_server() {
    // Fourteen rows leaves the inventory too short to fit even one card whole, so the
    // count of whole cards is zero. Publishing that as the viewport would hand the
    // extent a `max_position` one past the end and leave `cursor_in_window` with
    // nothing to pull, and the pin would collapse the offset straight back — the very
    // snap-back this change removes. The floor of one keeps the list scrollable.
    let mut app = language_server_app(16, 14);
    let hit = inventory_track(&app);
    let extent = hit.track.extent();
    assert_eq!(extent.viewport, 1, "a pane this short fits no card whole");
    assert_eq!(
        extent.max_position(),
        15,
        "every server must stay reachable"
    );

    let track = hit.track.rect();
    let (start, _) = hit.track.thumb_span().unwrap_or_default();
    press(&mut app, track.x, track.y + start);
    drag(&mut app, track.x, track.bottom() - 1);
    let (_, landed) = inventory(&app);
    assert_eq!(landed, 15, "the drag should reach the last server");

    screen(&mut app, 100, 14);
    assert_eq!(
        inventory(&app).1,
        15,
        "and it must not snap back on the next frame"
    );
}

#[test]
fn an_exactly_tiling_inventory_counts_every_card_it_paints() {
    // The boundary of the whole-card test. At 24 rows the cards tile the pane exactly,
    // so the bottom one ends flush with the content and no card is clipped — every
    // card painted is a card that fits, and the published viewport has to say so.
    // A predicate of `<` rather than `<=` would drop that flush card and understate
    // the window by one.
    let app = language_server_app(16, 24);
    let painted = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view.row_hits.len(),
        _ => panic!("expected the language-server manager"),
    };
    let extent = inventory_track(&app).track.extent();
    assert!(painted > 0, "the inventory should paint some cards");
    assert_eq!(
        extent.viewport, painted,
        "with nothing clipped, the viewport must count every painted card"
    );
}

#[test]
fn scrolling_the_inventory_leaves_a_selection_that_is_already_in_view_alone() {
    // `cursor_in_window` only pulls the cursor when it falls *outside* the window
    // asked for. A scroll that still contains the selected card must leave it put —
    // the selection is what the action strip acts on, so moving it needlessly would
    // retarget "Restart" behind the user's back.
    let mut app = language_server_app(16, 24);
    select_server(&mut app, 4);
    screen(&mut app, 100, 24);

    let extent = inventory_track(&app).track.extent();
    assert!(
        extent.viewport >= 2,
        "a one-card window cannot contain a selection off its top edge"
    );
    // The window [3, 3 + viewport) still holds card 4.
    app.scroll_surface_to(ScrollSurface::TabRows, 3, extent.viewport);

    assert_eq!(
        inventory(&app).0,
        4,
        "a window that still holds the selection must not move it"
    );
}
