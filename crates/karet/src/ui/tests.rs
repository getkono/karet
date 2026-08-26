use karet_core::LineCol;
use karet_core::Range;
use karet_session::RequestId;
use karet_session::SpellingHit;
use karet_widgets::breadcrumbs::breadcrumb_segment_spans;
use karet_widgets::textarea::TextAreaStyle;
use karet_widgets::textarea::cursor_row as commit_cursor_row;
use karet_widgets::textarea::styled_text;

use super::scm::change_line;
use super::*;
use crate::app::CommitInput;

#[test]
fn scm_change_rows_show_colored_added_and_removed_counts() {
    use karet_session::ChangeSummary;
    use karet_vcs::StatusKind;
    use ratatui::buffer::Buffer;
    use ratatui::widgets::Widget;

    let theme = Theme::dark();
    let line = change_line(
        &theme,
        &ChangeSummary {
            path: PathBuf::from("src/lib.rs"),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            added: 12,
            removed: 3,
        },
        (12, 3),
    );
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let added = text.find("+12").unwrap_or_default();
    let removed = text.find("\u{2212}3").unwrap_or_default();
    assert!(added > 0);
    assert!(removed > added);

    let area = Rect::new(0, 0, 40, 1);
    let mut buffer = Buffer::empty(area);
    Paragraph::new(line).render(area, &mut buffer);
    assert_eq!(
        buffer[(u16::try_from(added).unwrap_or_default(), 0)].fg,
        theme.role(ThemeRole::DiagnosticHint).to_ratatui()
    );
    assert_eq!(
        buffer[(u16::try_from(removed).unwrap_or_default(), 0)].fg,
        theme.role(ThemeRole::DiagnosticError).to_ratatui()
    );
}

#[test]
fn scrollable_lines_clamp_both_axes_and_draw_horizontal_position()
-> Result<(), std::convert::Infallible> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(6, 3);
    let mut terminal = Terminal::new(backend)?;
    let mut scroll = 9;
    let mut column = 3;
    terminal.draw(|frame| {
        draw_scrollable_lines(
            frame,
            &Theme::dark(),
            frame.area(),
            vec![Line::raw("0123456789")],
            &mut scroll,
            &mut column,
        );
    })?;

    assert_eq!(scroll, 0);
    assert_eq!(column, 3);
    // Both tracks are reserved out of the 6x3 area, so the text gets 5 columns and
    // 2 rows — the bars never sit on top of a character.
    let visible = (0..5)
        .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
        .collect::<String>();
    assert_eq!(visible, "34567");
    assert!(
        (0..5).any(|x| terminal.backend().buffer()[(x, 2)].symbol() != " "),
        "horizontal overflow should paint a bar in the reserved bottom row"
    );
    // The single line fits the two content rows, so the vertical track stays empty.
    assert!(
        (0..3).all(|y| terminal.backend().buffer()[(5, y)].symbol() == " "),
        "content that fits should not paint a vertical bar"
    );
    Ok(())
}

#[test]
fn a_markdown_preview_is_inset_from_its_pane_on_every_side() {
    let inner = markdown_preview_rect(Rect::new(10, 5, 40, 20));
    assert_eq!(inner, Rect::new(12, 6, 36, 18));
}

#[test]
fn markdown_link_hits_follow_wrapping_scrolling_and_wide_text() {
    let wrapped = karet_markdown::parse("[日本語 link](docs/readme.md)\n\nplain\n").wrap(8);
    let hits = markdown_link_hits(&wrapped, Rect::new(10, 5, 8, 2), 0);
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|hit| hit.target == "docs/readme.md"));
    assert!(
        hits.iter()
            .all(|hit| hit.rect.x >= 10 && hit.rect.right() <= 18)
    );
    assert!(hits.iter().any(|hit| hit.rect.width >= 6));

    let scrolled = markdown_link_hits(&wrapped, Rect::new(10, 5, 8, 1), 2);
    assert!(scrolled.is_empty());
}

#[test]
fn osc8_link_cells_are_self_contained_and_share_an_explicit_id() {
    let uri = "https://example.com";
    let id = osc8_id(uri);
    let first = osc8_symbol(uri, "x");
    let second = osc8_symbol(uri, "y");

    assert_eq!(
        first,
        format!("\u{1b}]8;id={id};{uri}\u{1b}\\x\u{1b}]8;;\u{1b}\\")
    );
    assert!(second.starts_with(&format!("\u{1b}]8;id={id};{uri}\u{1b}\\")));
    assert_ne!(id, osc8_id("https://example.org"));
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    );
}

#[test]
fn osc8_link_bytes_reach_the_crossterm_backend() -> Result<(), Box<dyn std::error::Error>> {
    use std::num::NonZeroU16;

    use ratatui::backend::Backend;
    use ratatui::backend::CrosstermBackend;
    use ratatui::buffer::Cell;
    use ratatui::buffer::CellDiffOption;

    let sequence = osc8_symbol("https://example.com", "x");
    let mut cell = Cell::default();
    cell.set_symbol(&sequence);
    if let Some(width) = NonZeroU16::new(1) {
        cell.set_diff_option(CellDiffOption::ForcedWidth(width));
    }
    let mut output = Vec::new();
    {
        let mut backend = CrosstermBackend::new(&mut output);
        backend.draw(std::iter::once((0, 0, &cell)))?;
        Backend::flush(&mut backend)?;
    }

    assert!(
        output
            .windows(sequence.len())
            .any(|window| window == sequence.as_bytes())
    );
    Ok(())
}

#[test]
fn commit_input_display_preserves_lines_and_marks_the_caret() {
    let mut input = CommitInput {
        text: "subject\nbody".to_string(),
        focused: true,
        ..CommitInput::default()
    };
    input.edit.set_cursor(&input.text, 8, false);
    let display = styled_text(
        &input.text,
        Some(input.edit.cursor()),
        input.edit.selection(),
        TextAreaStyle::default(),
    );
    assert_eq!(display.lines[0].to_string(), "subject");
    assert_eq!(display.lines[1].to_string(), "▏body");
    assert_eq!(commit_cursor_row(&input.text, input.edit.cursor(), 40), 1);
    assert_eq!(commit_cursor_row("abcdefghij", 10, 5), 2);
}

#[test]
fn text_field_display_styles_the_selected_run() {
    let mut edit = TextFieldState::default();
    edit.set_cursor("abcd", 1, false);
    edit.set_cursor("abcd", 3, true);
    let display = text_field_text(
        "abcd",
        &edit,
        true,
        Style::default().fg(Color::White),
        Style::default().fg(Color::White).bg(Color::Blue),
        Style::default().fg(Color::Red),
    );
    let spans = &display.lines[0].spans;
    assert_eq!(spans[0].style.bg, None);
    assert_eq!(spans[1].style.bg, Some(Color::Blue));
    assert_eq!(spans[2].style.bg, Some(Color::Blue));
    assert_eq!(spans[3].content, "▏");
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
fn relative_time_uses_compact_git_style_units() {
    let now = 40_000_000;
    assert_eq!(scm::relative_time_at(now + 1, now), "just now");
    assert_eq!(scm::relative_time_at(now - 42, now), "42s ago");
    assert_eq!(scm::relative_time_at(now - 120, now), "2m ago");
    assert_eq!(scm::relative_time_at(now - 10_800, now), "3h ago");
    assert_eq!(scm::relative_time_at(now - 172_800, now), "2d ago");
    assert_eq!(scm::relative_time_at(now - 1_209_600, now), "2w ago");
    assert_eq!(scm::relative_time_at(now - 5_184_000, now), "2mo ago");
    assert_eq!(scm::relative_time_at(now - 63_072_000, now), "2y ago");
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

    let titles = tab_display_titles(&tabs, root, karet_filetype::IconStyle::Unicode);

    assert_eq!(titles[0].prefix, "src/view/");
    assert_eq!(titles[0].name, "mod.rs");
    assert_eq!(titles[1].prefix, "tests/view/");
    assert_eq!(titles[1].name, "mod.rs");
    assert_eq!(titles[2].prefix, "");
    assert_eq!(titles[2].name, "lib.rs");
}

#[test]
fn symbolic_link_tabs_carry_the_configured_link_marker() {
    let mut tab = test_code_tab("/repo/alias.rs");
    tab.is_symlink = true;
    let titles = tab_display_titles(&[tab], Path::new("/repo"), karet_filetype::IconStyle::Ascii);
    assert_eq!(titles[0].name, "alias.rs @");
}

#[test]
fn markdown_tabs_expose_preview_and_table_actions() {
    let mut tab = test_code_tab("/repo/README.md");
    let actions = pane_actions(&tab);
    assert_eq!(actions.len(), 2);
    assert_eq!(
        actions[0],
        (UiIcon::Preview, Command::MarkdownPreviewSide, false)
    );
    assert_eq!(
        actions[1],
        (UiIcon::FormatTable, Command::FormatMarkdownTables, false)
    );

    tab.markdown_preview = Some(crate::tab::MarkdownPreviewState::default());
    assert!(pane_actions(&tab)[0].2);
    assert!(pane_actions(&test_code_tab("/repo/main.rs")).is_empty());
}

#[test]
fn tex_tabs_expose_the_external_build_preview_action() {
    assert_eq!(
        pane_actions(&test_code_tab("/repo/main.tex")),
        vec![(UiIcon::Preview, Command::LatexBuildPreview, false)]
    );
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
    let files = vec![crate::render::test_file_view(
        "src/main.rs",
        "fn a() {}\n",
        "fn b() {}\n",
    )];
    let width = 60u16;
    let theme = Theme::dark();
    let lines = changed_files_lines(&theme, &files, width);
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
    let changed = lines.iter().find(|line| line.style.bg.is_some());
    assert!(changed.is_some(), "the card contains a changed diff line");
    if let Some(changed) = changed {
        let changed_text: String = changed
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(changed_text.as_str()),
            usize::from(width),
            "the add/remove background reaches the card edge"
        );
        assert_eq!(
            changed.spans.last().and_then(|span| span.style.bg),
            changed.style.bg
        );
    }
}

#[test]
fn pane_diff_backgrounds_fill_unified_and_split_widths() {
    use ratatui::buffer::Buffer;
    use ratatui::widgets::Widget;

    let theme = Theme::dark();
    let file = crate::render::test_file_view("notes.txt", "old\n", "new\n");
    let mut lines = render::unified_lines(&file, &theme);
    render::pad_diff_lines(&mut lines, 40);
    let removed = lines
        .iter()
        .position(|line| line.style.bg == Some(theme.role(ThemeRole::DiffRemoved).to_ratatui()))
        .unwrap_or_default();
    let added = lines
        .iter()
        .position(|line| line.style.bg == Some(theme.role(ThemeRole::DiffAdded).to_ratatui()))
        .unwrap_or_default();
    assert_ne!(removed, added);

    let area = Rect::new(0, 0, 40, u16::try_from(lines.len()).unwrap_or(u16::MAX));
    let mut buffer = Buffer::empty(area);
    Paragraph::new(lines).render(area, &mut buffer);
    assert_eq!(
        buffer[(39, u16::try_from(removed).unwrap_or_default())].bg,
        theme.role(ThemeRole::DiffRemoved).to_ratatui()
    );
    assert_eq!(
        buffer[(39, u16::try_from(added).unwrap_or_default())].bg,
        theme.role(ThemeRole::DiffAdded).to_ratatui()
    );

    let (mut left, mut right) = render::side_by_side_lines(&file, &theme);
    render::pad_diff_lines(&mut left, 20);
    render::pad_diff_lines(&mut right, 20);
    let changed: Vec<_> = [left, right]
        .into_iter()
        .filter_map(|lines| lines.into_iter().find(|line| line.style.bg.is_some()))
        .collect();
    assert_eq!(changed.len(), 2);
    for changed in changed {
        let width = changed
            .spans
            .iter()
            .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
            .sum::<usize>();
        assert_eq!(width, 20);
        assert_eq!(
            changed.spans.last().and_then(|span| span.style.bg),
            changed.style.bg
        );
    }
}

#[test]
fn narrow_file_card_headers_never_exceed_the_pane() {
    use unicode_width::UnicodeWidthStr;

    let file =
        crate::render::test_file_view("very/long/\u{65e5}\u{672c}\u{8a9e}/filename.rs", "", "x\n");
    for width in 1..24u16 {
        let top = file_card(&Theme::dark(), &file, width)
            .into_iter()
            .next()
            .expect("a card header");
        let text: String = top.spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(
            UnicodeWidthStr::width(text.as_str()) <= usize::from(width),
            "header {text:?} exceeds width {width}"
        );
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
    let files = CommitFiles::default();
    let flat = |l: &Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };

    // Without a forge verdict, a signed commit reads "Signed"; the reported hit
    // must land exactly on that badge text within its line.
    let (lines, hit) = commit_detail_lines(&Theme::dark(), &detail, &files, false, 80);
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
    let (revealed, _) = commit_detail_lines(&Theme::dark(), &detail, &files, true, 80);
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

/// A unique scratch directory for a Spelling render test.
fn spelling_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "karet-spelling-{name}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// One scan hit for `path` at 0-based `line`.
fn spelling_hit(path: &Path, line: u32, col: u32, word: &str, line_text: &str) -> SpellingHit {
    SpellingHit {
        path: path.to_path_buf(),
        range: Range {
            start: LineCol::new(line, col),
            end: LineCol::new(line, col + word.chars().count() as u32),
        },
        word: word.to_owned(),
        line_text: line_text.to_owned(),
    }
}

#[test]
fn spelling_rows_render_the_word_its_line_number_and_context()
-> Result<(), std::convert::Infallible> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = spelling_dir("render");
    let path = dir.join("notes.md");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.spelling.hits = vec![spelling_hit(&path, 4, 4, "wrld", "the wrld ends")];
    app.spelling.rebuild_rows();
    app.spelling.files_scanned = 1;
    app.spelling.scanned = true;

    let mut terminal = Terminal::new(TestBackend::new(44, 6))?;
    let theme = app.theme.clone();
    let _ = terminal.draw(|frame| {
        let area = frame.area();
        super::sidebar::draw_spelling_panel(
            frame,
            &mut app,
            &theme,
            area,
            &mut ScrollHits::default(),
        );
    });
    let painted: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();

    assert!(painted.contains("notes.md"), "{painted}");
    assert!(painted.contains("wrld"), "{painted}");
    // 1-based in the UI, though the model is 0-based.
    assert!(painted.contains("5:"), "{painted}");
    assert!(painted.contains("the wrld ends"), "{painted}");
    assert!(painted.contains("⟳ scan"), "{painted}");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn an_unscanned_panel_invites_a_scan_rather_than_showing_an_empty_list()
-> Result<(), std::convert::Infallible> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = spelling_dir("empty");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.settings.spellcheck.enabled = true;

    let mut terminal = Terminal::new(TestBackend::new(48, 4))?;
    let theme = app.theme.clone();
    let render = |app: &mut App, terminal: &mut Terminal<TestBackend>| -> String {
        let _ = terminal.draw(|frame| {
            let area = frame.area();
            super::sidebar::draw_spelling_panel(
                frame,
                app,
                &theme,
                area,
                &mut ScrollHits::default(),
            );
        });
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    };

    assert!(render(&mut app, &mut terminal).contains("press ⟳ to scan"));

    app.spelling.scanning = Some(RequestId(1));
    assert!(render(&mut app, &mut terminal).contains("scanning"));

    app.spelling.scanning = None;
    app.spelling.files_scanned = 12;
    app.spelling.scanned = true;
    assert!(render(&mut app, &mut terminal).contains("no misspellings"));

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn the_activity_switcher_reserves_a_cell_pair_for_every_panel()
-> Result<(), std::convert::Infallible> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = spelling_dir("switcher");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.settings.spellcheck.enabled = true;
    let mut terminal = Terminal::new(TestBackend::new(40, 1))?;
    let theme = app.theme.clone();
    let mut header = |app: &mut App| {
        let _ = terminal.draw(|frame| {
            let area = frame.area();
            super::sidebar::draw_sidebar_header(frame, app, &theme, area);
        });
        app.panel_hits
            .iter()
            .map(|&(_, _, panel)| panel)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        header(&mut app),
        vec![
            SidebarPanel::Explorer,
            SidebarPanel::Search,
            SidebarPanel::SourceControl,
            SidebarPanel::Spelling,
            SidebarPanel::Todos,
            SidebarPanel::Debug,
        ],
        "every panel needs a switcher button, in activity-bar order"
    );

    // Spell check off retires the Spelling button along with its panel; the
    // codetag setting does the same for Todos.
    app.settings.spellcheck.enabled = false;
    app.settings.editor.semantic_comments.enabled = false;
    assert_eq!(
        header(&mut app),
        vec![
            SidebarPanel::Explorer,
            SidebarPanel::Search,
            SidebarPanel::SourceControl,
            SidebarPanel::Debug,
        ],
    );
    app.settings.editor.semantic_comments.enabled = true;
    app.settings.spellcheck.enabled = true;
    let _ = header(&mut app);
    // Two cells each, marching left to right and staying inside the header.
    for window in app.panel_hits.windows(2) {
        assert_eq!(window[1].0, window[0].1, "{:?}", app.panel_hits);
    }
    for &(start, end, _) in &app.panel_hits {
        assert_eq!(end - start, 2);
        assert!(
            end <= 40,
            "the switcher must fit the header: {start}..{end}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
