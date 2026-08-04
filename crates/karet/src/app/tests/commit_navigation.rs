#[test]
fn clicking_stacked_and_wide_file_rows_jumps_to_their_cards() {
    for width in [80, 104] {
        let mut app = responsive_commit_app();
        let _ = screen(&mut app, width, 16);
        let hit = app.pane_frames[0].commit_file_hits[1];
        app.handle_editor_click(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.rect.x.saturating_add(1),
            row: hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        let TabKind::Commit { view, .. } = &app.tabs[app.active].kind else {
            panic!("expected commit tab");
        };
        assert_eq!(view.scroll, hit.scroll, "failed at width {width}");
    }
}

#[test]
fn commit_file_keys_walk_card_anchors_without_wrapping() {
    let mut app = responsive_commit_app();
    let _ = screen(&mut app, 80, 12);
    let anchors = match &app.tabs[app.active].kind {
        TabKind::Commit { view, .. } => view.file_anchors.clone(),
        _ => panic!("expected commit tab"),
    };

    app.dispatch(Command::NextChangedFile);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::Commit { view, .. } if view.scroll == anchors[0]
    ));
    app.dispatch(Command::NextChangedFile);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::Commit { view, .. } if view.scroll == anchors[1]
    ));
    app.dispatch(Command::PrevChangedFile);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::Commit { view, .. } if view.scroll == anchors[0]
    ));
    app.dispatch(Command::PrevChangedFile);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::Commit { view, .. } if view.scroll == anchors[0]
    ));
}
