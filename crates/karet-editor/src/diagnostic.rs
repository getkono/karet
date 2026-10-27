//! Diagnostics shown in place: a severity sign in the gutter, the message at
//! the end of its line, and the styling a diagnostic's tags ask for.
//!
//! The signs and messages are built here as plain [`Decoration`]s and
//! [`VirtualText`]s, so they share the gutter's marker lane and the
//! end-of-line slot with every other producer on the same terms: the
//! application orders the lane and picks the slot priority.

use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::Diagnostic;
use karet_core::DiagnosticTag;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Severity;
use karet_core::VirtualText;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::text::col_in_range;

/// Which lines show their diagnostics' messages at the end of the line (see
/// [`diagnostic_messages`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InlineDiagnostics {
    /// No line does; the gutter sign and the underline remain.
    #[default]
    Off,
    /// Only lines a caret is on.
    CurrentLine,
    /// Every line a diagnostic starts on.
    All,
}

/// The theme role a diagnostic of `severity` is colored in.
pub use karet_core::severity_role;

/// The one-cell gutter sign for a diagnostic of `severity`.
#[must_use]
pub fn severity_sign(severity: Severity) -> char {
    match severity {
        Severity::Error => '\u{2718}',   // ✘
        Severity::Warning => '\u{25b2}', // ▲
        Severity::Hint => '\u{00b7}',    // ·
        _ => '\u{2022}',                 // •
    }
}

/// One gutter sign per line a diagnostic starts on, for the most severe
/// diagnostic starting there, in line order.
///
/// Each sign is a [`DecorationKind::GutterMarker`] colored by
/// [`severity_role`]; where it lands among the other markers of a line is the
/// order the caller supplies the decorations in (the first marker wins).
#[must_use]
pub fn diagnostic_signs(diagnostics: &[Diagnostic]) -> Vec<Decoration> {
    let mut worst: std::collections::BTreeMap<u32, Severity> = std::collections::BTreeMap::new();
    for diagnostic in diagnostics {
        worst
            .entry(diagnostic.range.start.line)
            .and_modify(|severity| *severity = (*severity).min(diagnostic.severity))
            .or_insert(diagnostic.severity);
    }
    worst
        .into_iter()
        .map(|(line, severity)| {
            Decoration::new(
                Range {
                    start: LineCol::new(line, 0),
                    end: LineCol::new(line, 0),
                },
                DecorationKind::GutterMarker {
                    glyph: severity_sign(severity),
                },
                Some(severity_role(severity)),
            )
        })
        .collect()
}

/// End-of-line messages for the diagnostics at least as severe as
/// `min_severity`, on the lines `mode` selects (`caret_lines` are the lines a
/// caret is on, for [`InlineDiagnostics::CurrentLine`]).
///
/// A diagnostic is shown on the line it starts on, by the first line of its
/// message. A line with several shows them most severe first (input order
/// among equals), each its own text in its severity's color and at
/// `priority`, so a short slot cuts the least severe with `…` first and drops
/// what gets no cells at all.
#[must_use]
pub fn diagnostic_messages(
    diagnostics: &[Diagnostic],
    mode: InlineDiagnostics,
    min_severity: Severity,
    caret_lines: &[u32],
    priority: u8,
) -> Vec<VirtualText> {
    let mut shown: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity <= min_severity)
        .filter(|diagnostic| match mode {
            InlineDiagnostics::Off => false,
            InlineDiagnostics::CurrentLine => caret_lines.contains(&diagnostic.range.start.line),
            InlineDiagnostics::All => true,
        })
        .collect();
    shown.sort_by_key(|diagnostic| (diagnostic.range.start.line, diagnostic.severity));
    shown
        .into_iter()
        .filter_map(|diagnostic| {
            let message = diagnostic.message.lines().next()?.trim();
            (!message.is_empty()).then(|| {
                VirtualText::end_of_line(
                    diagnostic.range.start.line,
                    format!("  {message}"),
                    severity_role(diagnostic.severity),
                    priority,
                )
            })
        })
        .collect()
}

/// The modifier that marks an underline as a diagnostic's while the editor
/// paints a frame, so [`finish_underlines`] can tell it from a navigable
/// symbol's. ratatui has no modifier for an underline's shape, and the marked
/// run must not merge with an equally colored plain-underlined one.
///
/// Blinking is never otherwise painted in the text, and the mark never leaves
/// the editor: [`finish_underlines`] strips it from every cell it covers.
pub(crate) const DIAGNOSTIC_UNDERLINE: Modifier = Modifier::RAPID_BLINK;

/// Turn the underline the cell already has curly (SGR `4:3`).
const CURLY: &str = "\u{1b}[4:3m";

/// Turn it straight again (SGR `4`), so the next cell starts with the
/// underline the backend believes is set and never inherits the curl.
const STRAIGHT: &str = "\u{1b}[4m";

/// End the frame's diagnostic underlines in `area`: strip the
/// [`DIAGNOSTIC_UNDERLINE`] mark from every cell, and with `undercurl` wrap
/// each marked cell's symbol in the SGR that curls its underline.
///
/// The backend writes styles before a cell's symbol and diffs them from the
/// cell before, so the wrapped symbol is self-contained: it curls the
/// underline the backend set for it and straightens it again, leaving the
/// terminal in the state the backend expects. Each cell is wrapped on its
/// own, since the backend repaints cells, not runs. The walk advances by each
/// symbol's width, and the wrapped cell keeps that width (`ForcedWidth`), so
/// the escape bytes never count as text.
pub(crate) fn finish_underlines(buf: &mut Buffer, area: Rect, undercurl: bool) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        let mut x = area.left();
        while x < area.right() {
            let Some(cell) = buf.cell_mut((x, y)) else {
                break;
            };
            let width = unicode_width::UnicodeWidthStr::width(cell.symbol()).max(1);
            let width = u16::try_from(width).unwrap_or(u16::MAX);
            if cell.modifier.contains(DIAGNOSTIC_UNDERLINE) {
                cell.modifier.remove(DIAGNOSTIC_UNDERLINE);
                if undercurl && cell.modifier.contains(Modifier::UNDERLINED) {
                    let symbol = format!("{CURLY}{}{STRAIGHT}", cell.symbol());
                    cell.set_symbol(&symbol);
                    if let Some(width) = std::num::NonZeroU16::new(width) {
                        cell.set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(width));
                    }
                }
            }
            x = x.saturating_add(width);
        }
    }
}

/// The modifiers the tags of the diagnostics covering column `col` on line
/// `l` add: [`DiagnosticTag::Deprecated`] strikes the text through and
/// [`DiagnosticTag::Unnecessary`] dims it.
pub(crate) fn tag_modifiers(diagnostics: &[Diagnostic], l: u32, col: u32) -> Modifier {
    diagnostics
        .iter()
        .filter(|diagnostic| col_in_range(l, col, diagnostic.range))
        .flat_map(|diagnostic| &diagnostic.tags)
        .fold(Modifier::empty(), |modifiers, tag| {
            modifiers
                | match tag {
                    DiagnosticTag::Deprecated => Modifier::CROSSED_OUT,
                    DiagnosticTag::Unnecessary => Modifier::DIM,
                    _ => Modifier::empty(),
                }
        })
}

#[cfg(test)]
mod tests {
    use karet_core::ThemeRole;
    use karet_core::VirtualTextPlacement;

    use super::*;

    fn diagnostic(line: u32, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: LineCol::new(line, 2),
                end: LineCol::new(line, 6),
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

    fn texts(messages: &[VirtualText]) -> Vec<(u32, &str, ThemeRole)> {
        messages
            .iter()
            .map(|text| (text.line, text.text.as_str(), text.role))
            .collect()
    }

    #[test]
    fn a_line_signs_its_most_severe_diagnostic_once() {
        let signs = diagnostic_signs(&[
            diagnostic(3, Severity::Hint, "h"),
            diagnostic(1, Severity::Warning, "w"),
            diagnostic(3, Severity::Error, "e"),
            diagnostic(3, Severity::Warning, "w"),
        ]);
        let signs: Vec<_> = signs
            .iter()
            .map(|sign| (sign.range.start.line, sign.kind.clone(), sign.role))
            .collect();
        assert_eq!(
            signs,
            [
                (
                    1,
                    DecorationKind::GutterMarker { glyph: '▲' },
                    Some(ThemeRole::DiagnosticWarning)
                ),
                (
                    3,
                    DecorationKind::GutterMarker { glyph: '✘' },
                    Some(ThemeRole::DiagnosticError)
                ),
            ]
        );
    }

    #[test]
    fn a_multi_line_diagnostic_signs_only_its_first_line() {
        let mut spans = diagnostic(2, Severity::Information, "i");
        spans.range.end = LineCol::new(5, 0);
        let signs = diagnostic_signs(&[spans]);
        assert_eq!(signs.len(), 1);
        assert_eq!(signs[0].range.start.line, 2);
        assert_eq!(signs[0].range.end.line, 2);
        assert_eq!(
            signs[0].kind,
            DecorationKind::GutterMarker { glyph: '•' },
            "information"
        );
    }

    #[test]
    fn every_severity_has_its_own_sign_and_color() {
        let severities = [
            Severity::Error,
            Severity::Warning,
            Severity::Information,
            Severity::Hint,
        ];
        let signs: Vec<char> = severities.into_iter().map(severity_sign).collect();
        assert_eq!(signs, ['✘', '▲', '•', '·']);
        assert_eq!(severity_role(Severity::Hint), ThemeRole::DiagnosticHint);
        assert_eq!(
            severity_role(Severity::Unknown),
            ThemeRole::DiagnosticInfo,
            "an unrecognised severity reads as information"
        );
    }

    #[test]
    fn messages_are_off_by_default() {
        let all = [diagnostic(0, Severity::Error, "boom")];
        assert_eq!(InlineDiagnostics::default(), InlineDiagnostics::Off);
        assert!(
            diagnostic_messages(&all, InlineDiagnostics::Off, Severity::Hint, &[0], 4).is_empty()
        );
    }

    #[test]
    fn current_line_shows_only_the_caret_lines() {
        let all = [
            diagnostic(0, Severity::Error, "first"),
            diagnostic(1, Severity::Error, "second"),
            diagnostic(2, Severity::Error, "third"),
        ];
        let shown = diagnostic_messages(
            &all,
            InlineDiagnostics::CurrentLine,
            Severity::Hint,
            &[2, 0],
            4,
        );
        assert_eq!(
            texts(&shown),
            [
                (0, "  first", ThemeRole::DiagnosticError),
                (2, "  third", ThemeRole::DiagnosticError),
            ]
        );
        let all_lines = diagnostic_messages(&all, InlineDiagnostics::All, Severity::Hint, &[], 4);
        assert_eq!(all_lines.len(), 3, "all ignores the carets");
    }

    #[test]
    fn a_line_lists_its_messages_most_severe_first_at_the_given_priority() {
        let all = [
            diagnostic(4, Severity::Hint, "consider"),
            diagnostic(4, Severity::Warning, "unused"),
            diagnostic(4, Severity::Error, "mismatched"),
            diagnostic(4, Severity::Warning, "shadowed"),
        ];
        let shown = diagnostic_messages(&all, InlineDiagnostics::All, Severity::Hint, &[], 7);
        assert_eq!(
            texts(&shown),
            [
                (4, "  mismatched", ThemeRole::DiagnosticError),
                (4, "  unused", ThemeRole::DiagnosticWarning),
                (4, "  shadowed", ThemeRole::DiagnosticWarning),
                (4, "  consider", ThemeRole::DiagnosticHint),
            ]
        );
        assert!(shown.iter().all(|text| text.placement
            == VirtualTextPlacement::EndOfLine {
                align: karet_core::EolAlign::Left,
                priority: 7,
            }));
    }

    #[test]
    fn the_threshold_drops_less_severe_diagnostics() {
        let all = [
            diagnostic(0, Severity::Hint, "hint"),
            diagnostic(0, Severity::Information, "info"),
            diagnostic(0, Severity::Warning, "warn"),
        ];
        let shown = diagnostic_messages(&all, InlineDiagnostics::All, Severity::Warning, &[], 0);
        assert_eq!(texts(&shown), [(0, "  warn", ThemeRole::DiagnosticWarning)]);
    }

    #[test]
    fn a_message_shows_its_first_line_and_an_empty_one_is_skipped() {
        let all = [
            diagnostic(0, Severity::Error, "expected `u32`\n  found `i64`"),
            diagnostic(1, Severity::Error, "\n"),
            diagnostic(2, Severity::Error, ""),
        ];
        let shown = diagnostic_messages(&all, InlineDiagnostics::All, Severity::Hint, &[], 0);
        assert_eq!(
            texts(&shown),
            [(0, "  expected `u32`", ThemeRole::DiagnosticError)]
        );
    }

    #[test]
    fn tags_strike_through_and_dim_only_the_cells_they_cover() {
        let mut deprecated = diagnostic(0, Severity::Hint, "old");
        deprecated.tags = vec![DiagnosticTag::Deprecated];
        let mut unnecessary = diagnostic(0, Severity::Hint, "unused");
        unnecessary.tags = vec![DiagnosticTag::Unnecessary, DiagnosticTag::Unknown];
        unnecessary.range.start.col = 4;
        unnecessary.range.end.col = 8;
        let all = [deprecated, unnecessary];
        assert_eq!(tag_modifiers(&all, 0, 1), Modifier::empty());
        assert_eq!(tag_modifiers(&all, 0, 2), Modifier::CROSSED_OUT);
        assert_eq!(
            tag_modifiers(&all, 0, 5),
            Modifier::CROSSED_OUT | Modifier::DIM,
            "overlapping tags combine"
        );
        assert_eq!(tag_modifiers(&all, 0, 7), Modifier::DIM);
        assert_eq!(tag_modifiers(&all, 1, 5), Modifier::empty());
    }
}
