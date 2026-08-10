use super::support::*;
use crate::app::*;

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
    assert!(!app.pane_frames[0].commit_collapse_hits.is_empty());

    let painted = screen(&mut app, 104, 16);
    let TabKind::Commit { view, .. } = &app.tabs[app.active].kind else {
        panic!("expected commit tab");
    };
    assert_eq!(view.layout, Some(crate::tab::CommitLayoutMode::Wide));
    assert_eq!(app.pane_frames[0].commit_file_hits.len(), 3);
    assert!(
        painted
            .iter()
            .any(|row| row.chars().nth(31) == Some('\u{2502}')),
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

    let wide = screen(&mut app, 104, 8);
    let TabKind::Commit { view, .. } = &app.tabs[app.active].kind else {
        panic!("expected commit tab");
    };
    assert_eq!(view.layout, Some(crate::tab::CommitLayoutMode::Wide));
    assert_eq!(view.scroll, view.file_anchors[1].saturating_add(1));
    assert_ne!(
        view.file_anchors[1], second,
        "wide layout removes the stacked TOC rows"
    );
    assert!(
        wide[1].contains("src/second.rs"),
        "the file header also sticks above the wide diff column"
    );
}

#[test]
fn file_header_disclosures_collapse_and_expand_cards_in_both_layouts() {
    for width in [80, 104] {
        let mut app = responsive_commit_app();
        let _ = screen(&mut app, width, 16);
        if let TabKind::Commit { view, .. } = &mut app.tabs[app.active].kind {
            view.scroll = view.file_anchors[0].saturating_add(1);
        }
        let sticky = screen(&mut app, width, 16);
        let content_top = app.pane_frames[0].content_rect.y;
        let hit = app.pane_frames[0]
            .commit_collapse_hits
            .iter()
            .find(|hit| hit.file == 0 && hit.rect.y == content_top)
            .copied();
        assert!(hit.is_some(), "missing sticky disclosure at width {width}");
        let Some(hit) = hit else { continue };
        assert!(sticky[usize::from(content_top)].contains("src/first.rs"));

        let before_second = match &app.tabs[app.active].kind {
            TabKind::Commit { view, .. } => view.file_anchors[1],
            _ => 0,
        };
        app.handle_editor_click(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.rect.x,
            row: hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        let collapsed = screen(&mut app, width, 16);
        let (after_second, anchor) = match &app.tabs[app.active].kind {
            TabKind::Commit { view, .. } => {
                assert!(view.collapsed_files.contains(&0));
                (view.file_anchors[1], view.file_anchors[0])
            },
            _ => (u16::MAX, 0),
        };
        assert!(
            after_second < before_second,
            "card body stayed visible at width {width}"
        );
        assert!(
            collapsed
                .iter()
                .any(|row| row.contains("\u{25b8}") && row.contains("src/first.rs"))
        );

        let expand = app.pane_frames[0]
            .commit_collapse_hits
            .iter()
            .find(|hit| hit.file == 0)
            .copied();
        assert!(expand.is_some());
        if let Some(expand) = expand {
            app.handle_editor_click(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: expand.rect.x,
                row: expand.rect.y,
                modifiers: KeyModifiers::NONE,
            });
        }
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::Commit { view, .. }
                if !view.collapsed_files.contains(&0) && view.scroll == anchor
        ));
    }
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
