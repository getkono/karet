//! The editor scrollbar's overview markers: where a long document's problems,
//! uncommitted changes, search matches and selections are, at a glance.

use karet_core::CursorState;
use karet_core::Range;
use karet_widgets::scroll::ScrollMarker;

use super::*;

/// The markers for one code tab's vertical scrollbar, in buffer lines, ordered
/// least important first so the painter lets the more important one win a shared
/// cell: selections, then search matches, then changed lines, then problems from
/// hint up to error.
pub(super) fn scroll_markers<'a>(
    theme: &Theme,
    problems: &[Diagnostic],
    changes: &[Decoration],
    matches: impl IntoIterator<Item = &'a Decoration>,
    cursors: &CursorState,
) -> Vec<ScrollMarker> {
    let mut markers: Vec<ScrollMarker> = cursors
        .selections
        .iter()
        .filter(|selection| !selection.is_empty())
        .map(|selection| lines(selection.range(), theme.style(ThemeRole::Selection)))
        .collect();
    let search = theme.style(ThemeRole::SearchMatch);
    markers.extend(matches.into_iter().map(|found| lines(found.range, search)));
    markers.extend(
        changes
            .iter()
            .filter_map(|change| Some(lines(change.range, theme.style(change.role?)))),
    );
    let mut problems: Vec<&Diagnostic> = problems.iter().collect();
    problems.sort_by_key(|problem| std::cmp::Reverse(problem.severity));
    markers.extend(problems.into_iter().map(|problem| {
        let role = karet_editor::severity_role(problem.severity);
        lines(problem.range, theme.style(role))
    }));
    markers
}

/// A marker over every line `range` touches, its end line included.
fn lines(range: Range, style: Style) -> ScrollMarker {
    let start = range.start.line as usize;
    let end = (range.end.line as usize).max(start) + 1;
    ScrollMarker::new(start, end, style)
}

#[cfg(test)]
mod tests {
    use karet_core::DecorationKind;
    use karet_core::LineCol;
    use karet_core::Selection;

    use super::*;

    fn range(first: u32, last: u32) -> Range {
        Range {
            start: LineCol::new(first, 0),
            end: LineCol::new(last, 1),
        }
    }

    fn diagnostic(line: u32, severity: Severity) -> Diagnostic {
        Diagnostic {
            range: range(line, line),
            severity,
            message: String::new(),
            source: None,
            code: None,
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    fn decoration(first: u32, last: u32, role: Option<ThemeRole>) -> Decoration {
        Decoration {
            range: range(first, last),
            kind: DecorationKind::TextBackground,
            role,
        }
    }

    #[test]
    fn markers_layer_selections_under_matches_under_changes_under_problems() {
        let theme = Theme::default();
        let cursors = CursorState {
            selections: vec![
                Selection::caret(LineCol::new(1, 0)),
                Selection {
                    anchor: LineCol::new(9, 0),
                    head: LineCol::new(7, 2),
                },
            ],
            primary: 0,
        };
        let markers = scroll_markers(
            &theme,
            &[
                diagnostic(40, Severity::Error),
                diagnostic(41, Severity::Hint),
            ],
            &[
                decoration(20, 22, Some(ThemeRole::GutterAdded)),
                decoration(23, 23, None),
            ],
            &[decoration(30, 30, Some(ThemeRole::SearchMatch))],
            &cursors,
        );

        let style = |role| theme.style(role);
        assert_eq!(
            markers,
            vec![
                ScrollMarker::new(7, 10, style(ThemeRole::Selection)),
                ScrollMarker::new(30, 31, style(ThemeRole::SearchMatch)),
                ScrollMarker::new(20, 23, style(ThemeRole::GutterAdded)),
                ScrollMarker::new(41, 42, style(ThemeRole::DiagnosticHint)),
                ScrollMarker::new(40, 41, style(ThemeRole::DiagnosticError)),
            ]
        );
    }

    #[test]
    fn an_inverted_range_still_marks_its_start_line() {
        let marker = lines(
            Range {
                start: LineCol::new(5, 0),
                end: LineCol::new(2, 0),
            },
            Style::default(),
        );

        assert_eq!((marker.start, marker.end), (5, 6));
    }
}
