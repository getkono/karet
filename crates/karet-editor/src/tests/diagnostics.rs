//! Diagnostics painted in place: the tag styling of the cells they cover, and
//! the gutter signs and end-of-line messages built from them.

use super::*;

/// A diagnostic over columns `start..end` of `line`.
fn diagnostic(
    line: u32,
    (start, end): (u32, u32),
    severity: Severity,
    message: &str,
) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: LineCol::new(line, start),
            end: LineCol::new(line, end),
        },
        severity,
        message: message.to_owned(),
        source: None,
        code: None,
        code_href: None,
        tags: Vec::new(),
        related: Vec::new(),
    }
}

/// Render `text` into `area` with `diagnostics`, and with the signs and
/// messages built from them supplied as decorations and virtual text.
fn render(text: &str, diagnostics: &[Diagnostic], messages: &[VirtualText], area: Rect) -> Buffer {
    let buffer = TextBuffer::from_text(text);
    let signs = diagnostic_signs(diagnostics);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .diagnostics(diagnostics)
        .decorations(&signs)
        .virtual_text(messages)
        .render(area, &mut target, &mut state);
    target
}

/// The row `y` as painted.
fn row(target: &Buffer, y: u16) -> String {
    (target.area.x..target.area.right())
        .map(|x| target[(x, y)].symbol().chars().next().unwrap_or(' '))
        .collect()
}

#[test]
fn a_deprecated_range_is_struck_through_and_an_unnecessary_one_dimmed() {
    let mut deprecated = diagnostic(0, (0, 3), Severity::Hint, "deprecated");
    deprecated.tags = vec![DiagnosticTag::Deprecated];
    let mut unnecessary = diagnostic(0, (4, 7), Severity::Hint, "unused");
    unnecessary.tags = vec![DiagnosticTag::Unnecessary];
    let target = render(
        "old bad ok\n",
        &[deprecated, unnecessary],
        &[],
        Rect::new(0, 0, 20, 1),
    );
    // Gutter: marker, digit, space; text starts at x = 3.
    let modifiers = |x: u16| target[(x, 0)].modifier;
    for x in 3..6 {
        assert!(modifiers(x).contains(Modifier::CROSSED_OUT), "column {x}");
        assert!(!modifiers(x).contains(Modifier::DIM), "column {x}");
    }
    for x in 7..10 {
        assert!(modifiers(x).contains(Modifier::DIM), "column {x}");
        assert!(!modifiers(x).contains(Modifier::CROSSED_OUT), "column {x}");
    }
    assert!(
        modifiers(3).contains(Modifier::UNDERLINED),
        "the tag adds to the underline rather than replacing it"
    );
    for x in [6, 11, 12] {
        assert!(
            !modifiers(x).intersects(Modifier::DIM | Modifier::CROSSED_OUT),
            "column {x} carries no tag"
        );
    }
}

#[test]
fn an_untagged_diagnostic_only_underlines() {
    let target = render(
        "let x\n",
        &[diagnostic(0, (4, 5), Severity::Error, "boom")],
        &[],
        Rect::new(0, 0, 12, 1),
    );
    let cell = &target[(7, 0)];
    assert!(cell.modifier.contains(Modifier::UNDERLINED));
    assert!(
        !cell
            .modifier
            .intersects(Modifier::DIM | Modifier::CROSSED_OUT)
    );
}

#[test]
fn each_line_shows_its_most_severe_sign_in_the_severity_color() {
    let theme = Theme::dark();
    let diagnostics = [
        diagnostic(0, (0, 1), Severity::Warning, "w"),
        diagnostic(0, (2, 3), Severity::Error, "e"),
        diagnostic(2, (0, 1), Severity::Hint, "h"),
    ];
    let target = render("a b\nc\nd\n", &diagnostics, &[], Rect::new(0, 0, 8, 3));
    assert_eq!(target[(0, 0)].symbol(), "✘");
    assert_eq!(
        target[(0, 0)].fg,
        theme.role(ThemeRole::DiagnosticError).to_ratatui()
    );
    assert_eq!(target[(0, 1)].symbol(), " ", "no diagnostic, no sign");
    assert_eq!(target[(0, 2)].symbol(), "·");
    assert_eq!(
        target[(0, 2)].fg,
        theme.role(ThemeRole::DiagnosticHint).to_ratatui()
    );
}

#[test]
fn an_earlier_marker_keeps_the_lane_from_a_sign() {
    let buffer = TextBuffer::from_text("a\n");
    let breakpoint = Decoration::new(
        Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 0),
        },
        DecorationKind::GutterMarker { glyph: '●' },
        Some(ThemeRole::Breakpoint),
    );
    let mut decorations = vec![breakpoint];
    decorations.extend(diagnostic_signs(&[diagnostic(
        0,
        (0, 1),
        Severity::Error,
        "e",
    )]));
    let area = Rect::new(0, 0, 6, 1);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).decorations(&decorations).render(
        area,
        &mut target,
        &mut EditorState::new(),
    );
    assert_eq!(target[(0, 0)].symbol(), "●");
}

#[test]
fn messages_trail_their_line_most_severe_first_and_cut_short_with_an_ellipsis() {
    let diagnostics = [
        diagnostic(0, (0, 1), Severity::Warning, "unused variable"),
        diagnostic(0, (0, 1), Severity::Error, "mismatched types"),
    ];
    let messages =
        diagnostic_messages(&diagnostics, InlineDiagnostics::All, Severity::Hint, &[], 1);
    let wide = render("let x;\n", &diagnostics, &messages, Rect::new(0, 0, 60, 1));
    assert_eq!(
        row(&wide, 0).trim_end(),
        "✘1 let x;  mismatched types  unused variable"
    );
    let theme = Theme::dark();
    assert_eq!(
        wide[(12, 0)].fg,
        theme.role(ThemeRole::DiagnosticError).to_ratatui()
    );
    assert_eq!(
        wide[(30, 0)].fg,
        theme.role(ThemeRole::DiagnosticWarning).to_ratatui()
    );

    let narrow = render("let x;\n", &diagnostics, &messages, Rect::new(0, 0, 32, 1));
    assert_eq!(
        row(&narrow, 0),
        "✘1 let x;  mismatched types  un…",
        "the least severe message is the one cut"
    );
}

/// `text` on one row of `width` with an Error over the `diagnosed` columns
/// and a navigable-symbol underline over the `navigable` ones, painted with
/// or without `undercurl`.
fn render_underlines(
    text: &str,
    width: u16,
    (diagnosed, navigable): ((u32, u32), (u32, u32)),
    undercurl: bool,
) -> Buffer {
    let buffer = TextBuffer::from_text(text);
    let diagnostics = [diagnostic(0, diagnosed, Severity::Error, "boom")];
    let link = [Decoration::new(
        Range {
            start: LineCol::new(0, navigable.0),
            end: LineCol::new(0, navigable.1),
        },
        DecorationKind::Underline,
        None,
    )];
    let area = Rect::new(0, 0, width, 1);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .diagnostics(&diagnostics)
        .decorations(&link)
        .undercurl(undercurl)
        .render(area, &mut target, &mut EditorState::new());
    target
}

/// The SGR-wrapped symbol a curled cell holding `symbol` carries.
fn curled(symbol: &str) -> String {
    format!("\u{1b}[4:3m{symbol}\u{1b}[4m")
}

#[test]
fn without_undercurl_a_diagnostic_keeps_a_plain_colored_underline() {
    let target = render_underlines("ab cd\n", 12, ((0, 2), (3, 5)), false);
    let error = Theme::dark().role(ThemeRole::DiagnosticError).to_ratatui();
    // Gutter: marker, digit, space; text starts at x = 3.
    for (x, symbol) in [(3, "a"), (4, "b")] {
        let cell = &target[(x, 0)];
        assert!(cell.modifier.contains(Modifier::UNDERLINED), "column {x}");
        assert_eq!(cell.underline_color, error, "column {x}");
        assert_eq!(cell.symbol(), symbol, "column {x} carries no escape");
    }
    assert!(
        (0..12).all(|x| !target[(x, 0)].modifier.contains(Modifier::RAPID_BLINK)),
        "the diagnostic mark never leaves the editor"
    );
}

#[test]
fn with_undercurl_only_the_diagnostic_cells_are_curled() {
    let target = render_underlines("ab cd\n", 12, ((0, 2), (3, 5)), true);
    assert_eq!(target[(3, 0)].symbol(), curled("a"));
    assert_eq!(target[(4, 0)].symbol(), curled("b"));
    assert!(target[(3, 0)].modifier.contains(Modifier::UNDERLINED));
    assert_eq!(target[(5, 0)].symbol(), " ", "between the two: untouched");
    for (x, symbol) in [(6, "c"), (7, "d")] {
        let cell = &target[(x, 0)];
        assert!(cell.modifier.contains(Modifier::UNDERLINED), "column {x}");
        assert_eq!(
            cell.symbol(),
            symbol,
            "a navigable underline stays straight"
        );
    }
    assert!(
        (0..12).all(|x| !target[(x, 0)].modifier.contains(Modifier::RAPID_BLINK)),
        "the mark is stripped when the underline curls too"
    );
}

#[test]
fn a_curled_wide_glyph_is_wrapped_once_and_keeps_its_width() {
    let target = render_underlines("界x\n", 12, ((0, 2), (5, 5)), true);
    assert_eq!(target[(3, 0)].symbol(), curled("界"));
    assert_eq!(
        target[(4, 0)].symbol(),
        " ",
        "the continuation cell is not wrapped again"
    );
    assert_eq!(target[(5, 0)].symbol(), curled("x"));
    assert!(
        matches!(
            target[(3, 0)].diff_option,
            ratatui::buffer::CellDiffOption::ForcedWidth(width) if width.get() == 2
        ),
        "the escape bytes do not count as text: the glyph stays two cells wide"
    );
}
