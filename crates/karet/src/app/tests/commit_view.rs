fn responsive_commit_files() -> Vec<FileView> {
    ["src/first.rs", "src/second.rs", "src/third.rs"]
        .into_iter()
        .map(|path| {
            FileView::new(
                change(path, StatusKind::Modified),
                crate::render::Section::Staged,
                false,
            )
        })
        .collect()
}

fn responsive_commit_app() -> App {
    let mut app = app();
    app.sidebar_visible = false;
    app.focus = Focus::Editor;
    app.push_tab(Tab::commit(
        Box::new(commit_detail(&"a".repeat(40), "responsive commit")),
        responsive_commit_files(),
    ));
    app
}

#[test]
fn commit_view_switches_at_104_columns_and_records_file_rows() {
    let mut app = responsive_commit_app();
    let _ = screen(&mut app, 103, 16);
    let TabKind::Commit { view, .. } = &app.tabs[app.active].kind else {
        panic!("expected commit tab");
    };
    assert_eq!(view.layout, Some(crate::tab::CommitLayoutMode::Stacked));
    assert_eq!(view.file_anchors.len(), 3);
    assert_eq!(app.pane_frames[0].commit_file_hits.len(), 3);

    let painted = screen(&mut app, 104, 16);
    let TabKind::Commit { view, .. } = &app.tabs[app.active].kind else {
        panic!("expected commit tab");
    };
    assert_eq!(view.layout, Some(crate::tab::CommitLayoutMode::Wide));
    assert_eq!(app.pane_frames[0].commit_file_hits.len(), 3);
    assert!(
        painted.iter().any(|row| row.chars().nth(31) == Some('\u{2502}')),
        "the 31-column rail is followed by its divider at the breakpoint"
    );
}

#[test]
fn stacked_sticky_header_and_resize_preserve_the_visible_file() {
    let mut app = responsive_commit_app();
    let _ = screen(&mut app, 80, 8);
    let second = match &mut app.tabs[app.active].kind {
        TabKind::Commit { view, .. } => {
            view.scroll = view.file_anchors[1].saturating_add(1);
            view.file_anchors[1]
        },
        _ => panic!("expected commit tab"),
    };
    let stacked = screen(&mut app, 80, 8);
    assert!(
        stacked[1].contains("src/second.rs"),
        "the active file header sticks to the content's first row"
    );

    let _ = screen(&mut app, 104, 8);
    let TabKind::Commit { view, .. } = &app.tabs[app.active].kind else {
        panic!("expected commit tab");
    };
    assert_eq!(view.layout, Some(crate::tab::CommitLayoutMode::Wide));
    assert_eq!(view.scroll, view.file_anchors[1].saturating_add(1));
    assert_ne!(view.file_anchors[1], second, "wide layout removes the stacked TOC rows");
}

#[test]
fn compare_view_uses_the_same_responsive_layout() {
    let mut app = app();
    app.sidebar_visible = false;
    app.push_tab(Tab::compare(
        "main".to_string(),
        "HEAD".to_string(),
        true,
        responsive_commit_files(),
    ));
    let _ = screen(&mut app, 104, 12);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::Compare { view, .. }
            if view.layout == Some(crate::tab::CommitLayoutMode::Wide)
                && view.file_anchors.len() == 3
    ));
}
