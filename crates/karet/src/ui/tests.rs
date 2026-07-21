use super::*;

#[test]
fn a_markdown_preview_is_inset_from_its_pane_on_every_side() {
    let inner = markdown_preview_rect(Rect::new(10, 5, 40, 20));
    assert_eq!(inner, Rect::new(12, 6, 36, 18));
}

#[test]
fn breadcrumb_spans_map_segments_and_leave_separator_gaps_unmapped() {
    let components = vec!["/".to_string(), "home".to_string(), "u".to_string()];
    let spans = breadcrumb_segment_spans(&components);
    // "/" + "  ›  " (5 cells) + "home" + "  ›  " + "u"
    assert_eq!(spans, vec![(0, 1), (6, 10), (15, 16)]);
    // The separator gap between spans belongs to no segment.
    assert!(spans.iter().all(|&(s, e)| !(s <= 3 && 3 < e)));
}

#[test]
fn breadcrumb_spans_use_display_width_for_wide_characters() {
    // "日本語" occupies 6 terminal cells, not 3.
    let components = vec!["\u{65e5}\u{672c}\u{8a9e}".to_string(), "a.rs".to_string()];
    assert_eq!(
        breadcrumb_segment_spans(&components),
        vec![(0, 6), (11, 15)]
    );
}

#[test]
fn breadcrumb_spans_of_no_components_are_empty() {
    assert!(breadcrumb_segment_spans(&[]).is_empty());
}

#[test]
fn a_pane_too_small_to_pad_paints_nothing_rather_than_to_the_edge() {
    // The padding needs 4 columns and 2 rows; below that there is no content rect.
    assert_eq!(markdown_preview_rect(Rect::new(0, 0, 4, 1)).height, 0);
    assert_eq!(markdown_preview_rect(Rect::new(0, 0, 3, 2)).width, 0);
    // Exactly enough for the padding leaves an empty — but valid — content rect.
    assert_eq!(markdown_preview_rect(Rect::new(0, 0, 4, 2)).width, 0);
}

fn test_code_tab(path: &str) -> Tab {
    use karet_text::TextBuffer;

    let buffer = TextBuffer::from_text("");
    Tab::new(
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path),
        TabKind::Code {
            path: PathBuf::from(path),
            language: "plaintext",
            doc: None,
            next_version: 0,
            buffer,
            text: String::new(),
            highlights: karet_syntax::Highlights::default(),
            semantic_blocks: karet_syntax::SemanticBlocks::default(),
            folds: karet_syntax::FoldRegions::default(),
            folded: std::collections::BTreeSet::new(),
            decos: Vec::new(),
            search_decos: Vec::new(),
            syntax_errors: Vec::new(),
        },
    )
}

#[test]
fn tab_titles_disambiguate_duplicate_file_names() {
    let root = Path::new("/repo");
    let tabs = vec![
        test_code_tab("/repo/src/view/mod.rs"),
        test_code_tab("/repo/tests/view/mod.rs"),
        test_code_tab("/repo/src/lib.rs"),
    ];

    let titles = tab_display_titles(&tabs, root);

    assert_eq!(titles[0].prefix, "src/view/");
    assert_eq!(titles[0].name, "mod.rs");
    assert_eq!(titles[1].prefix, "tests/view/");
    assert_eq!(titles[1].name, "mod.rs");
    assert_eq!(titles[2].prefix, "");
    assert_eq!(titles[2].name, "lib.rs");
}

#[test]
fn active_tab_prefix_keeps_active_fill() {
    let theme = Theme::dark();
    let base = tab_text_style(&theme, true, true, false);

    let prefix = tab_prefix_style(&theme, base, true, true);

    assert_eq!(prefix.fg, Some(theme.role(ThemeRole::Muted).to_ratatui()));
    assert_eq!(
        prefix.bg,
        Some(theme.role(ThemeRole::Foreground).to_ratatui())
    );
    assert!(!prefix.add_modifier.contains(Modifier::REVERSED));
    assert!(prefix.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn unfocused_active_tab_prefix_stays_muted_without_fill() {
    let theme = Theme::dark();
    let base = tab_text_style(&theme, true, false, false);

    assert_eq!(
        base.fg,
        Some(theme.role(ThemeRole::DiagnosticInfo).to_ratatui())
    );
    let prefix = tab_prefix_style(&theme, base, true, false);

    assert_eq!(prefix.fg, Some(theme.role(ThemeRole::Muted).to_ratatui()));
    assert_eq!(prefix.bg, None);
    assert!(prefix.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn chrome_button_hover_changes_foreground_without_background() {
    let theme = Theme::dark();
    let hover = chrome_button_style(&theme, ChromeButtonState::Hovered);
    assert_eq!(
        hover.fg,
        Some(theme.role(ThemeRole::LineNumberActive).to_ratatui())
    );
    assert_eq!(hover.bg, None);

    let active_hover = chrome_button_style(&theme, ChromeButtonState::ActiveHovered);
    assert_eq!(
        active_hover.fg,
        Some(theme.role(ThemeRole::Foreground).to_ratatui())
    );
    assert_eq!(active_hover.bg, None);
    assert!(active_hover.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn format_datetime_is_correct_and_applies_offset() {
    assert_eq!(format_datetime(0, 0), "1970-01-01 00:00");
    assert_eq!(format_datetime(0, 3600), "1970-01-01 01:00");
    // 1_700_000_000 = 2023-11-14 22:13:20 UTC.
    assert_eq!(format_datetime(1_700_000_000, 0), "2023-11-14 22:13");
}

#[test]
fn verified_badge_reflects_forge_and_signature() {
    use karet_vcs::CommitSignature;
    use karet_vcs::SignatureKind;
    let verified = karet_session::GithubVerification {
        verified: true,
        reason: "valid".to_string(),
        signer: None,
    };
    let unverified = karet_session::GithubVerification {
        verified: false,
        reason: "unsigned".to_string(),
        signer: None,
    };
    let sig = CommitSignature {
        kind: SignatureKind::Ssh,
        signer_key: None,
        raw: String::new(),
    };
    assert_eq!(verified_badge(Some(&verified), None).1, "Verified");
    assert_eq!(verified_badge(Some(&unverified), None).1, "Unverified");
    assert_eq!(verified_badge(None, Some(&sig)).1, "Signed");
    assert_eq!(verified_badge(None, None).1, "Unsigned");
}

#[test]
fn file_cards_are_boxed_and_width_sized() {
    use karet_vcs::FileChange;
    use karet_vcs::StatusKind;
    let change = FileChange {
        path: std::path::PathBuf::from("src/main.rs"),
        old_path: None,
        status: StatusKind::Modified,
        is_binary: false,
        old: "fn a() {}\n".to_string(),
        new: "fn b() {}\n".to_string(),
    };
    let files = vec![render::FileView::new(
        change,
        render::Section::Staged,
        false,
    )];
    let width = 60u16;
    let lines = changed_files_lines(&Theme::dark(), &files, width);
    let text: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();

    // A rounded top rule (corners) and a bottom rule bound the card.
    let top = text
        .iter()
        .find(|t| t.starts_with('\u{256d}'))
        .expect("a top rule");
    assert!(top.contains("src/main.rs"), "top rule carries the path");
    assert!(top.ends_with('\u{256e}'), "top rule closes with a corner");
    assert_eq!(
        top.chars().count(),
        usize::from(width),
        "the top rule spans the pane width"
    );
    let bottom = text
        .iter()
        .find(|t| t.starts_with('\u{2570}') && t.ends_with('\u{256f}'))
        .expect("a bottom rule");
    assert_eq!(bottom.chars().count(), usize::from(width));
    // Diff body lines sit behind a left rail.
    assert!(
        text.iter().any(|t| t.starts_with("\u{2502} ")),
        "diff lines are railed"
    );
}

#[test]
fn narrow_file_card_headers_never_exceed_the_pane() {
    use karet_vcs::FileChange;
    use karet_vcs::StatusKind;
    use unicode_width::UnicodeWidthStr;

    let file = render::FileView::new(
        FileChange {
            path: PathBuf::from("very/long/\u{65e5}\u{672c}\u{8a9e}/filename.rs"),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            old: String::new(),
            new: "x\n".to_string(),
        },
        render::Section::Staged,
        false,
    );
    for width in 1..24u16 {
        let top = file_card(&Theme::dark(), &file, width)
            .into_iter()
            .next()
            .expect("a card header");
        let text: String = top.spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(UnicodeWidthStr::width(text.as_str()) <= usize::from(width));
    }
    assert_eq!(
        truncate_start("a/\u{65e5}\u{672c}\u{8a9e}/file.rs", 8),
        "\u{2026}file.rs"
    );
}

#[test]
fn badge_hit_spans_the_badge_and_reveal_explains_it() {
    use karet_vcs::CommitDetail;
    use karet_vcs::CommitSignature;
    use karet_vcs::Identity;
    use karet_vcs::SignatureKind;

    let id = || Identity {
        name: "Tester".to_string(),
        email: "t@example.com".to_string(),
        time: 0,
        offset: 0,
    };
    let detail = CommitDetail {
        hash: "a".repeat(40),
        short_hash: "aaaaaaa".to_string(),
        summary: "subject".to_string(),
        body: String::new(),
        author: id(),
        committer: id(),
        parents: Vec::new(),
        signature: Some(CommitSignature {
            kind: SignatureKind::Ssh,
            signer_key: None,
            raw: String::new(),
        }),
    };
    let files: Vec<render::FileView> = Vec::new();
    let flat = |l: &Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };

    // Without a forge verdict, a signed commit reads "Signed"; the reported hit
    // must land exactly on that badge text within its line.
    let (lines, hit) = commit_detail_lines(
        &Theme::dark(),
        &detail,
        &files,
        CommitFileStatus::Ready,
        None,
        false,
        80,
    );
    let hit = hit.expect("a signed commit has a badge");
    let chars: Vec<char> = flat(&lines[hit.line as usize]).chars().collect();
    let span: String = chars[hit.col as usize..(hit.col + hit.width) as usize]
        .iter()
        .collect();
    assert!(
        span.contains("Signed"),
        "the hit span covers the badge: {span:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| flat(l).contains("cryptographic signature")),
        "no explanation is shown until revealed"
    );

    // Revealing inserts the badge's plain-language meaning.
    let (revealed, _) = commit_detail_lines(
        &Theme::dark(),
        &detail,
        &files,
        CommitFileStatus::Ready,
        None,
        true,
        80,
    );
    assert!(
        revealed
            .iter()
            .any(|l| flat(l).contains("cryptographic signature")),
        "the reveal explains what Signed means"
    );
}

#[test]
fn cursor_status_label_reports_position_and_selection_extent() {
    use karet_core::LineCol;
    use karet_text::TextBuffer;

    let buffer = TextBuffer::from_text("hello\nworld\n");
    let mut tab = Tab::new(
        "a.txt",
        TabKind::Code {
            path: std::path::PathBuf::from("/x/a.txt"),
            language: "plaintext",
            doc: None,
            next_version: 0,
            buffer: buffer.clone(),
            text: "hello\nworld\n".to_string(),
            highlights: karet_syntax::Highlights::default(),
            semantic_blocks: karet_syntax::SemanticBlocks::default(),
            folds: karet_syntax::FoldRegions::default(),
            folded: std::collections::BTreeSet::new(),
            decos: Vec::new(),
            search_decos: Vec::new(),
            syntax_errors: Vec::new(),
        },
    );

    tab.editor.place_caret(LineCol::new(1, 2));
    assert_eq!(cursor_status_label(&tab), "Ln 2, Col 3");

    // A same-line selection reports the selected character count.
    tab.editor
        .set_selection(&buffer, LineCol::new(0, 1), LineCol::new(0, 4));
    assert_eq!(cursor_status_label(&tab), "Ln 1, Col 5 (3 selected)");

    // A multi-line selection reports the line count instead.
    tab.editor
        .set_selection(&buffer, LineCol::new(0, 0), LineCol::new(1, 2));
    assert_eq!(cursor_status_label(&tab), "Ln 2, Col 3 (2 lines selected)");
}

#[test]
fn welcome_hints_are_all_bound() {
    // Every welcome-screen command must resolve a chord from the keymap;
    // otherwise the cheat-sheet would silently drop it. The status bar's hints
    // are now enumerated from the keymap directly, so they can't drift.
    for &(cmd, _) in WELCOME_HINTS {
        assert!(
            keymap::hint_for(cmd, ChordStyle::Verbose).is_some(),
            "welcome command {cmd:?} has no keymap binding"
        );
    }
}

#[test]
fn hint_bar_is_context_aware() {
    use crate::keymap::FocusTarget;
    let cmds = |ctx| {
        keymap::hints_for(ctx, ChordStyle::Caret)
            .iter()
            .map(|h| h.command)
            .collect::<Vec<_>>()
    };
    let editor = cmds(Context::focus(FocusTarget::Editor));
    let scm = cmds(Context::focus(FocusTarget::SourceControl));
    // The bar's command set follows the focused pane.
    assert!(editor.contains(&Command::Save));
    assert!(!editor.contains(&Command::ScmStage));
    assert!(scm.contains(&Command::ScmStage));
    assert!(!scm.contains(&Command::Save));
}

#[test]
fn pack_hints_respects_width() {
    let hint = |chord: &str, command, verb| keymap::Hint {
        chord: chord.to_string(),
        command,
        verb,
    };
    let hints = vec![
        hint("^S", Command::Save, "save"),
        hint("^Z", Command::Undo, "undo"),
        hint("^C", Command::Copy, "copy"),
    ];
    // A wide bar shows everything; a zero-width bar shows nothing.
    assert_eq!(pack_hints(&hints, 100), 3);
    assert_eq!(pack_hints(&hints, 0), 0);
    // A narrow bar drops trailing hints (leaving room for the ` +N` marker).
    assert!(pack_hints(&hints, 12) < hints.len());
}
