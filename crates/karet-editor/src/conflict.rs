//! Merge-conflict marker recognition for editor decoration.

use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::ThemeRole;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MarkerKind {
    Start,
    Base,
    Separator,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Marker {
    kind: MarkerKind,
    width: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Conflict {
    start: usize,
    base: Option<usize>,
    separator: usize,
    end: usize,
}

enum ParseOutcome {
    Complete(Conflict),
    Invalid { resume_at: usize },
}

/// Build neutral editor decorations for complete Git-style merge conflicts.
///
/// Both ordinary two-way conflicts and `diff3` conflicts with a `|||||||` base
/// section are recognized. Marker runs must begin in column zero, contain at
/// least seven characters, and use one consistent width across the conflict.
/// Incomplete, mismatched, or nested marker sets are deliberately ignored so
/// ordinary source text is not partially highlighted as a conflict.
#[must_use]
pub fn conflict_decorations(text: &str) -> Vec<Decoration> {
    let lines: Vec<&str> = text.lines().collect();
    let mut decorations = Vec::new();
    let mut line = 0;

    while line < lines.len() {
        let Some(Marker {
            kind: MarkerKind::Start,
            width,
        }) = marker(lines[line])
        else {
            line += 1;
            continue;
        };

        match parse_conflict(&lines, line, width) {
            ParseOutcome::Complete(conflict) => {
                decorate_conflict(&mut decorations, conflict);
                line = conflict.end + 1;
            },
            ParseOutcome::Invalid { resume_at } => line = resume_at,
        }
    }

    decorations
}

fn marker(line: &str) -> Option<Marker> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let first = line.chars().next()?;
    let kind = match first {
        '<' => MarkerKind::Start,
        '|' => MarkerKind::Base,
        '=' => MarkerKind::Separator,
        '>' => MarkerKind::End,
        _ => return None,
    };
    let width = line.bytes().take_while(|byte| *byte == first as u8).count();
    if width < 7 {
        return None;
    }
    let suffix = &line[width..];
    let valid_suffix = match kind {
        MarkerKind::Separator => suffix.trim().is_empty(),
        MarkerKind::Start | MarkerKind::Base | MarkerKind::End => {
            suffix.is_empty() || suffix.starts_with(char::is_whitespace)
        },
    };
    valid_suffix.then_some(Marker { kind, width })
}

fn parse_conflict(lines: &[&str], start: usize, width: usize) -> ParseOutcome {
    let mut base = None;
    let mut separator = None;
    for (line, content) in lines.iter().enumerate().skip(start + 1) {
        let Some(found) = marker(content) else {
            continue;
        };
        if found.kind == MarkerKind::Start {
            let resume_at =
                nested_conflict_end(lines, line, width).map_or(start + 1, |end| end + 1);
            return ParseOutcome::Invalid { resume_at };
        }
        if found.width != width {
            return ParseOutcome::Invalid {
                resume_at: start + 1,
            };
        }
        match (found.kind, separator) {
            (MarkerKind::Base, None) if base.is_none() => base = Some(line),
            (MarkerKind::Separator, None) => separator = Some(line),
            (MarkerKind::End, Some(separator)) => {
                return ParseOutcome::Complete(Conflict {
                    start,
                    base,
                    separator,
                    end: line,
                });
            },
            _ => {
                return ParseOutcome::Invalid {
                    resume_at: start + 1,
                };
            },
        }
    }
    ParseOutcome::Invalid {
        resume_at: start + 1,
    }
}

fn nested_conflict_end(lines: &[&str], nested_start: usize, width: usize) -> Option<usize> {
    let mut depth = 2_u32;
    for (line, content) in lines.iter().enumerate().skip(nested_start + 1) {
        let Some(found) = marker(content).filter(|found| found.width == width) else {
            continue;
        };
        match found.kind {
            MarkerKind::Start => depth = depth.saturating_add(1),
            MarkerKind::End => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(line);
                }
            },
            MarkerKind::Base | MarkerKind::Separator => {},
        }
    }
    None
}

fn decorate_conflict(decorations: &mut Vec<Decoration>, conflict: Conflict) {
    marker_decorations(decorations, conflict.start, '<');
    if let Some(base) = conflict.base {
        section_decoration(
            decorations,
            conflict.start + 1,
            base,
            ThemeRole::DiffModified,
        );
        marker_decorations(decorations, base, '|');
        section_decoration(
            decorations,
            base + 1,
            conflict.separator,
            ThemeRole::DiffRemoved,
        );
    } else {
        section_decoration(
            decorations,
            conflict.start + 1,
            conflict.separator,
            ThemeRole::DiffModified,
        );
    }
    marker_decorations(decorations, conflict.separator, '=');
    section_decoration(
        decorations,
        conflict.separator + 1,
        conflict.end,
        ThemeRole::DiffAdded,
    );
    marker_decorations(decorations, conflict.end, '>');
}

fn marker_decorations(decorations: &mut Vec<Decoration>, line: usize, glyph: char) {
    let range = line_range(line);
    decorations.push(Decoration {
        range,
        kind: DecorationKind::GutterMarker { glyph },
        role: Some(ThemeRole::DiffModified),
    });
    decorations.push(Decoration {
        range,
        kind: DecorationKind::LineBackground,
        role: Some(ThemeRole::DiffModified),
    });
}

fn section_decoration(
    decorations: &mut Vec<Decoration>,
    start: usize,
    end: usize,
    role: ThemeRole,
) {
    if start >= end {
        return;
    }
    decorations.push(Decoration {
        range: Range {
            start: LineCol::new(start as u32, 0),
            end: LineCol::new((end - 1) as u32, u32::MAX),
        },
        kind: DecorationKind::LineBackground,
        role: Some(role),
    });
}

fn line_range(line: usize) -> Range {
    Range {
        start: LineCol::new(line as u32, 0),
        end: LineCol::new(line as u32, u32::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backgrounds(decorations: &[Decoration]) -> Vec<(u32, u32, ThemeRole)> {
        decorations
            .iter()
            .filter_map(|decoration| match decoration.kind {
                DecorationKind::LineBackground => decoration
                    .role
                    .map(|role| (decoration.range.start.line, decoration.range.end.line, role)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn decorates_complete_two_way_conflict() {
        let decorations = conflict_decorations(
            "before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\nafter\n",
        );
        assert_eq!(
            backgrounds(&decorations),
            vec![
                (1, 1, ThemeRole::DiffModified),
                (2, 2, ThemeRole::DiffModified),
                (3, 3, ThemeRole::DiffModified),
                (4, 4, ThemeRole::DiffAdded),
                (5, 5, ThemeRole::DiffModified),
            ]
        );
        let glyphs: Vec<char> = decorations
            .iter()
            .filter_map(|decoration| match decoration.kind {
                DecorationKind::GutterMarker { glyph } => Some(glyph),
                _ => None,
            })
            .collect();
        assert_eq!(glyphs, ['<', '=', '>']);
    }

    #[test]
    fn decorates_diff3_base_separately() {
        let decorations = conflict_decorations(
            "<<<<<<< ours\ncurrent\n||||||| base\nancestor\n=======\nincoming\n>>>>>>> theirs\n",
        );
        assert_eq!(
            backgrounds(&decorations),
            vec![
                (0, 0, ThemeRole::DiffModified),
                (1, 1, ThemeRole::DiffModified),
                (2, 2, ThemeRole::DiffModified),
                (3, 3, ThemeRole::DiffRemoved),
                (4, 4, ThemeRole::DiffModified),
                (5, 5, ThemeRole::DiffAdded),
                (6, 6, ThemeRole::DiffModified),
            ]
        );
    }

    #[test]
    fn accepts_consistent_custom_marker_width() {
        let decorations =
            conflict_decorations("<<<<<<<<< ours\na\n=========\nb\n>>>>>>>>> theirs\n");
        assert!(!decorations.is_empty());
    }

    #[test]
    fn ignores_incomplete_mismatched_and_nested_markers() {
        for text in [
            "<<<<<<< ours\na\n=======\nb\n",
            "<<<<<<< ours\na\n========\nb\n>>>>>>> theirs\n",
            "<<<<<<< outer\n<<<<<<< inner\na\n=======\nb\n>>>>>>> inner\n>>>>>>> outer\n",
            "let shift = <<<<<<< value;\n",
        ] {
            assert!(conflict_decorations(text).is_empty(), "unexpected: {text}");
        }
    }

    #[test]
    fn finds_multiple_independent_conflicts() {
        let text =
            "<<<<<<< a\n1\n=======\n2\n>>>>>>> b\nplain\n<<<<<<< c\n3\n=======\n4\n>>>>>>> d\n";
        let decorations = conflict_decorations(text);
        assert_eq!(
            decorations
                .iter()
                .filter(|decoration| matches!(decoration.kind, DecorationKind::GutterMarker { .. }))
                .count(),
            6
        );
    }
}
