//! Decoration styling and overlap: a decoration's foreground and modifiers,
//! and which of several overlapping decorations a cell shows.

use karet_core::Emphasis;
use ratatui::style::Color;

use super::*;

/// The text column `col` of row 0 sits after the two-cell gutter (marker and
/// fold columns) `LineNumbers::Off` leaves.
const GUTTER: u16 = 2;

fn span(first: u32, last: u32) -> Range {
    Range {
        start: LineCol::new(0, first),
        end: LineCol::new(0, last),
    }
}

fn background(first: u32, last: u32, role: ThemeRole) -> Decoration {
    Decoration::new(
        span(first, last),
        DecorationKind::TextBackground,
        Some(role),
    )
}

fn style(first: u32, last: u32) -> Decoration {
    Decoration::new(span(first, last), DecorationKind::TextStyle, None)
}

fn render(buffer: &TextBuffer, decorations: &[Decoration], state: &mut EditorState) -> Buffer {
    let area = Rect::new(0, 0, 16, 1);
    let mut target = Buffer::empty(area);
    Editor::new(buffer)
        .theme(&Theme::dark())
        .decorations(decorations)
        .line_numbers(LineNumbers::Off)
        .render(area, &mut target, state);
    target
}

fn color(role: ThemeRole) -> Color {
    Theme::dark().role(role).to_ratatui()
}

fn cell(target: &Buffer, col: u16) -> &ratatui::buffer::Cell {
    &target[(GUTTER + col, 0)]
}

#[test]
fn a_higher_priority_background_wins_whatever_its_order() {
    let buffer = TextBuffer::from_text("abcdef");
    let decorations = [
        background(0, 4, ThemeRole::SearchMatch),
        background(2, 6, ThemeRole::DiffAdded).with_priority(1),
    ];
    let target = render(&buffer, &decorations, &mut EditorState::new());
    assert_eq!(cell(&target, 1).bg, color(ThemeRole::SearchMatch));
    assert_eq!(cell(&target, 2).bg, color(ThemeRole::DiffAdded));
    assert_eq!(cell(&target, 3).bg, color(ThemeRole::DiffAdded));
    assert_eq!(cell(&target, 5).bg, color(ThemeRole::DiffAdded));
}

/// The behaviour every producer had before priorities existed: at equal
/// priority the decoration supplied first keeps the cell.
#[test]
fn equal_priorities_keep_the_first_supplied() {
    let buffer = TextBuffer::from_text("abcdef");
    let decorations = [
        background(0, 4, ThemeRole::SearchMatch),
        background(2, 6, ThemeRole::DiffAdded),
    ];
    let target = render(&buffer, &decorations, &mut EditorState::new());
    assert_eq!(cell(&target, 3).bg, color(ThemeRole::SearchMatch));
    assert_eq!(cell(&target, 4).bg, color(ThemeRole::DiffAdded));
}

#[test]
fn a_text_style_recolours_its_text_and_paints_no_background() {
    let buffer = TextBuffer::from_text("abcdef");
    let theme = Theme::dark();
    let decorations = [
        style(0, 2).with_fg(TokenId::KEYWORD),
        style(2, 4).with_fg(ThemeRole::DiagnosticError),
    ];
    let plain = render(&buffer, &[], &mut EditorState::new());
    let target = render(&buffer, &decorations, &mut EditorState::new());
    assert_eq!(
        cell(&target, 0).fg,
        theme.color(TokenId::KEYWORD).to_ratatui()
    );
    assert_eq!(cell(&target, 3).fg, color(ThemeRole::DiagnosticError));
    assert_eq!(cell(&target, 4).fg, cell(&plain, 4).fg);
    for col in 0..6 {
        assert_eq!(cell(&target, col).bg, cell(&plain, col).bg, "column {col}");
    }
}

#[test]
fn the_highest_priority_foreground_wins_an_overlap() {
    let buffer = TextBuffer::from_text("abcdef");
    let decorations = [
        style(0, 6).with_fg(ThemeRole::DiffAdded),
        style(2, 4).with_fg(ThemeRole::DiffRemoved).with_priority(5),
    ];
    let target = render(&buffer, &decorations, &mut EditorState::new());
    assert_eq!(cell(&target, 1).fg, color(ThemeRole::DiffAdded));
    assert_eq!(cell(&target, 2).fg, color(ThemeRole::DiffRemoved));
    assert_eq!(cell(&target, 4).fg, color(ThemeRole::DiffAdded));
}

/// Modifiers blend rather than compete: a low-priority bold and a
/// high-priority italic both reach the cells they share.
#[test]
fn overlapping_modifiers_blend_across_priorities() {
    let buffer = TextBuffer::from_text("abcdef");
    let decorations = [
        style(0, 4).with_modifiers(Emphasis::BOLD),
        background(2, 6, ThemeRole::SearchMatch)
            .with_modifiers(Emphasis::ITALIC | Emphasis::STRIKETHROUGH)
            .with_priority(9),
    ];
    let target = render(&buffer, &decorations, &mut EditorState::new());
    assert_eq!(cell(&target, 0).modifier, Modifier::BOLD);
    assert_eq!(
        cell(&target, 3).modifier,
        Modifier::BOLD | Modifier::ITALIC | Modifier::CROSSED_OUT
    );
    assert_eq!(
        cell(&target, 5).modifier,
        Modifier::ITALIC | Modifier::CROSSED_OUT
    );
}

/// The underline colour is single-valued: a later, higher-priority underline
/// takes the cells it shares with an earlier one, and at equal priority the
/// earlier underline keeps them.
#[test]
fn the_highest_priority_underline_colours_an_overlap() {
    let buffer = TextBuffer::from_text("abcdef");
    let underline = |first, last, role| {
        Decoration::new(span(first, last), DecorationKind::Underline, Some(role))
    };
    let raised = [
        underline(0, 4, ThemeRole::DiagnosticWarning),
        underline(2, 6, ThemeRole::DiagnosticError).with_priority(1),
    ];
    let target = render(&buffer, &raised, &mut EditorState::new());
    assert_eq!(
        cell(&target, 1).underline_color,
        color(ThemeRole::DiagnosticWarning)
    );
    assert_eq!(
        cell(&target, 3).underline_color,
        color(ThemeRole::DiagnosticError)
    );
    assert_eq!(
        cell(&target, 5).underline_color,
        color(ThemeRole::DiagnosticError)
    );

    let tie = [
        underline(0, 4, ThemeRole::DiagnosticWarning),
        underline(2, 6, ThemeRole::DiagnosticError),
    ];
    let target = render(&buffer, &tie, &mut EditorState::new());
    assert_eq!(
        cell(&target, 3).underline_color,
        color(ThemeRole::DiagnosticWarning)
    );
    assert_eq!(
        cell(&target, 4).underline_color,
        color(ThemeRole::DiagnosticError)
    );
}

/// A line background styles every cell of its lines, not just its columns.
#[test]
fn a_line_background_styles_the_whole_line() {
    let buffer = TextBuffer::from_text("abcdef\nghi");
    let line = Decoration::new(
        Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 0),
        },
        DecorationKind::LineBackground,
        Some(ThemeRole::DebugStoppedLine),
    )
    .with_fg(ThemeRole::Muted)
    .with_modifiers(Emphasis::DIM);
    let target = render(&buffer, &[line], &mut EditorState::new());
    let last = cell(&target, 5);
    assert_eq!(last.bg, color(ThemeRole::DebugStoppedLine));
    assert_eq!(last.fg, color(ThemeRole::Muted));
    assert_eq!(last.modifier, Modifier::DIM);
}

/// A gutter marker's style fields are documented as ignored: they never
/// leak onto the line's text.
#[test]
fn a_gutter_markers_style_leaves_the_text_alone() {
    let buffer = TextBuffer::from_text("abcdef");
    let marker = Decoration::new(
        span(0, 6),
        DecorationKind::GutterMarker { glyph: '●' },
        Some(ThemeRole::Breakpoint),
    )
    .with_fg(ThemeRole::DiffRemoved)
    .with_modifiers(Emphasis::BOLD);
    let plain = render(&buffer, &[], &mut EditorState::new());
    let target = render(&buffer, &[marker], &mut EditorState::new());
    assert_eq!(target[(0, 0)].symbol(), "●");
    assert_eq!(target[(0, 0)].fg, color(ThemeRole::Breakpoint));
    for col in 0..6 {
        assert_eq!(cell(&target, col), cell(&plain, col), "column {col}");
    }
}

#[test]
fn a_higher_priority_gutter_marker_takes_the_lane() {
    let buffer = TextBuffer::from_text("abc");
    let marker = |glyph| {
        Decoration::new(
            span(0, 0),
            DecorationKind::GutterMarker { glyph },
            Some(ThemeRole::Muted),
        )
    };
    let first = render(
        &buffer,
        &[marker('a'), marker('b')],
        &mut EditorState::new(),
    );
    assert_eq!(first[(0, 0)].symbol(), "a");
    let raised = render(
        &buffer,
        &[marker('a'), marker('b').with_priority(1)],
        &mut EditorState::new(),
    );
    assert_eq!(raised[(0, 0)].symbol(), "b");
}

/// A swatch keeps its tie with a background, as it always has, however the
/// two are ordered; a higher-priority background takes the cell, and the
/// swatch's contrasting foreground goes with the swatch.
#[test]
fn a_swatch_wins_a_tie_with_a_background_but_not_a_higher_priority() {
    let buffer = TextBuffer::from_text("abcdef");
    let swatch = Decoration::new(
        span(0, 6),
        DecorationKind::ColorSwatch {
            rgba: [0xff, 0xff, 0xff, 0xff],
        },
        None,
    );
    let swatch_bg = Color::Rgb(0xff, 0xff, 0xff);
    let tie = [background(0, 6, ThemeRole::SearchMatch), swatch.clone()];
    let target = render(&buffer, &tie, &mut EditorState::new());
    assert_eq!(cell(&target, 2).bg, swatch_bg);
    assert_eq!(cell(&target, 2).fg, Color::Rgb(0, 0, 0));

    let raised = [
        background(0, 6, ThemeRole::SearchMatch).with_priority(1),
        swatch,
    ];
    let plain = render(&buffer, &[], &mut EditorState::new());
    let target = render(&buffer, &raised, &mut EditorState::new());
    assert_eq!(cell(&target, 2).bg, color(ThemeRole::SearchMatch));
    assert_eq!(cell(&target, 2).fg, cell(&plain, 2).fg);
}

/// A selection replaces a decoration's background but keeps its foreground,
/// as it keeps a syntax colour.
#[test]
fn a_selection_keeps_a_decorations_foreground() {
    let buffer = TextBuffer::from_text("abcdef");
    let decorations = [background(0, 6, ThemeRole::SearchMatch).with_fg(ThemeRole::DiffAdded)];
    let mut state = EditorState::new();
    state.set_selection(&buffer, LineCol::new(0, 0), LineCol::new(0, 3));
    let target = render(&buffer, &decorations, &mut state);
    assert_eq!(cell(&target, 1).bg, color(ThemeRole::Selection));
    assert_eq!(cell(&target, 1).fg, color(ThemeRole::DiffAdded));
    assert_eq!(cell(&target, 4).bg, color(ThemeRole::SearchMatch));
}

/// The index answers per line, so decorations far off screen neither show
/// nor hide what is on it.
#[test]
fn decorations_on_other_lines_never_reach_a_row() {
    let buffer = TextBuffer::from_text("abc\ndef");
    let far: Vec<Decoration> = (1..500)
        .map(|line| {
            Decoration::new(
                Range {
                    start: LineCol::new(line, 0),
                    end: LineCol::new(line, 3),
                },
                DecorationKind::TextBackground,
                Some(ThemeRole::DiffRemoved),
            )
            .with_priority(100)
        })
        .chain(std::iter::once(background(0, 3, ThemeRole::SearchMatch)))
        .collect();
    let target = render(&buffer, &far, &mut EditorState::new());
    assert_eq!(cell(&target, 1).bg, color(ThemeRole::SearchMatch));
}
