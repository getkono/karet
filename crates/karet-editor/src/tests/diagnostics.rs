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
    let breakpoint = Decoration {
        range: Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 0),
        },
        kind: DecorationKind::GutterMarker { glyph: '●' },
        role: Some(ThemeRole::Breakpoint),
    };
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
