use super::*;

fn file(path: &str, old: &str, new: &str) -> render::FileView {
    crate::render::test_file_view(path, old, new)
}

#[test]
fn file_document_windows_match_the_complete_document() {
    let theme = Theme::dark();
    let files = vec![
        file("src/a.rs", "one\ntwo\n", "one\nchanged\n"),
        file("src/b.rs", "old\n", "new\nmore\n"),
    ];
    let width = 72;
    let collapsed = BTreeSet::new();
    let doc = build_files(
        &theme,
        &files,
        width,
        true,
        CommitFileStatus::Ready,
        &collapsed,
    );
    let mut complete = doc.prefix.clone();
    for file in &files {
        complete.push(Line::raw(""));
        complete.push(file_card_header(&theme, file, width, false));
        complete.extend(file_card_body(&theme, file, 0, usize::MAX, width));
        complete.push(file_card_footer(&theme, width));
    }
    assert_eq!(usize::from(doc.rows), complete.len());
    for start in 0..complete.len() {
        let actual = visible_file_lines(
            &theme,
            &files,
            width,
            &doc,
            u16::try_from(start).unwrap_or(u16::MAX),
            4,
            &collapsed,
        );
        let expected = complete
            .iter()
            .skip(start)
            .take(4)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "window starting at row {start}");
    }
}

#[test]
fn collapsed_file_document_keeps_only_its_disclosure_header() {
    let theme = Theme::dark();
    let files = vec![file("src/a.rs", "one\ntwo\n", "one\nchanged\n")];
    let width = 72;
    let expanded = build_files(
        &theme,
        &files,
        width,
        true,
        CommitFileStatus::Ready,
        &BTreeSet::new(),
    );
    let toggled_files = BTreeSet::from([0]);
    let collapsed = build_files(
        &theme,
        &files,
        width,
        true,
        CommitFileStatus::Ready,
        &toggled_files,
    );
    assert!(collapsed.rows < expanded.rows);
    let lines = visible_file_lines(
        &theme,
        &files,
        width,
        &collapsed,
        collapsed.anchors[0],
        2,
        &toggled_files,
    );
    assert_eq!(lines.len(), 1);
    assert!(lines[0].to_string().contains("\u{25b8}"));
}

#[test]
fn a_generated_file_starts_collapsed_and_a_source_file_does_not() {
    let theme = Theme::dark();
    let files = vec![
        file("Cargo.lock", "a = 1\n", "a = 2\n"),
        file("src/a.rs", "one\ntwo\n", "one\nchanged\n"),
    ];
    // Nothing toggled: the lockfile still folds, the source file does not.
    let doc = build_files(
        &theme,
        &files,
        72,
        true,
        CommitFileStatus::Ready,
        &BTreeSet::new(),
    );
    let lockfile = visible_file_lines(
        &theme,
        &files,
        72,
        &doc,
        doc.anchors[0],
        1,
        &BTreeSet::new(),
    );
    assert!(
        lockfile[0].to_string().contains("\u{25b8}"),
        "{:?}",
        lockfile[0]
    );

    let source = visible_file_lines(
        &theme,
        &files,
        72,
        &doc,
        doc.anchors[1],
        1,
        &BTreeSet::new(),
    );
    assert!(
        source[0].to_string().contains("\u{25be}"),
        "{:?}",
        source[0]
    );
}

#[test]
fn toggling_a_generated_file_expands_it() {
    let theme = Theme::dark();
    let files = [file("Cargo.lock", "a = 1\n", "a = 2\n")];
    let collapsed = build_files(
        &theme,
        &files,
        72,
        true,
        CommitFileStatus::Ready,
        &BTreeSet::new(),
    );
    // The override set flips the default rather than naming the collapsed set,
    // so the same entry that collapses a source file expands a lockfile.
    let toggled = BTreeSet::from([0]);
    let expanded = build_files(&theme, &files, 72, true, CommitFileStatus::Ready, &toggled);
    assert!(expanded.rows > collapsed.rows);
    let lines = visible_file_lines(
        &theme,
        &files,
        72,
        &expanded,
        expanded.anchors[0],
        2,
        &toggled,
    );
    assert!(lines[0].to_string().contains("\u{25be}"));
}

#[test]
fn a_generated_card_names_its_reason() {
    let theme = Theme::dark();
    let files = [file("Cargo.lock", "a = 1\n", "a = 2\n")];
    let header = file_card_header(&theme, &files[0], 72, true).to_string();
    assert!(header.contains("(lockfile)"), "{header}");
    assert!(
        file_index_line(&theme, &files[0], 72, false)
            .to_string()
            .contains("(lockfile)")
    );
}

#[test]
fn a_narrow_card_drops_the_reason_before_the_path() {
    let theme = Theme::dark();
    let files = [file("Cargo.lock", "a = 1\n", "a = 2\n")];
    let header = file_card_header(&theme, &files[0], 24, true).to_string();
    assert!(!header.contains("(lockfile)"), "{header}");
    // The path survives (truncated from the start) once the reason is gone.
    assert!(header.contains(".lock"), "{header}");
}

#[test]
fn a_card_header_never_outgrows_its_width() {
    // The reason is extra content on an already width-budgeted line; it must
    // not push the closing rule past the pane.
    let theme = Theme::dark();
    let files = [file("Cargo.lock", "a = 1\n", "a = 2\n")];
    for width in [13u16, 20, 24, 30, 40, 72, 120] {
        let header = file_card_header(&theme, &files[0], width, true);
        assert!(
            line_width(&header) <= usize::from(width),
            "width {width}: {header:?}"
        );
    }
}

#[test]
fn collapse_hit_tracks_horizontal_scroll() {
    let area = Rect::new(10, 4, 20, 5);
    let hit = collapse_hit(area, 2, 6, 2);
    assert_eq!(hit.map(|hit| hit.rect), Some(Rect::new(11, 6, 1, 1)));
    assert!(collapse_hit(area, 2, 6, 4).is_none());
}
